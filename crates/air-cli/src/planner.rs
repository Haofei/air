use crate::models::call_openai_model;
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) struct PlanOptions {
    pub(crate) task: Option<String>,
    pub(crate) task_file: Option<PathBuf>,
    pub(crate) store: PathBuf,
    pub(crate) model_config: PathBuf,
    pub(crate) planner_model: String,
    pub(crate) allow_internal: bool,
    pub(crate) output: Option<PathBuf>,
}

pub(crate) struct ValidatePlanOptions {
    pub(crate) plan: Option<PathBuf>,
    pub(crate) profile: Option<PathBuf>,
    pub(crate) store: Option<PathBuf>,
}

pub(crate) fn plan_task(options: PlanOptions) -> Result<()> {
    let PlanOptions {
        task,
        task_file,
        store,
        model_config,
        planner_model,
        allow_internal,
        output,
    } = options;

    let task = match (task, task_file) {
        (Some(task), None) => task,
        (None, Some(path)) => fs::read_to_string(path)?,
        (None, None) => anyhow::bail!("one of --task or --task-file is required"),
        (Some(_), Some(_)) => anyhow::bail!("use either --task or --task-file, not both"),
    };

    let store_path = store;
    let store = air_linker::parse_module_store_file(&store_path)?;
    let base_dir = module_base_dir_for_store_path(&store, &store_path);
    let catalog = module_catalog(&store, &base_dir, allow_internal)?;
    let recipes = recipe_catalog(&store, &base_dir, allow_internal)?;
    let request = planner_request(&task, &store, catalog, recipes, allow_internal);

    let plan_value = call_openai_model(model_config, &planner_model, &request)?;
    let mut plan = parse_planner_response(plan_value, &store, &base_dir, allow_internal)?;
    normalize_planner_run_plan(&mut plan);
    let report = air_linker::validate_run_plan(&plan, &store, &base_dir);
    if !report.is_success() {
        for diagnostic in &report.diagnostics {
            eprintln!("error[{}]: {}", diagnostic.code, diagnostic.message);
        }
        eprintln!("generated run plan:\n{}", serde_yaml::to_string(&plan)?);
        anyhow::bail!("planner generated an invalid run plan");
    }

    let yaml = serde_yaml::to_string(&plan)?;
    if let Some(output) = output {
        fs::write(output, yaml)?;
    } else {
        print!("{yaml}");
    }

    Ok(())
}

pub(crate) fn planner_request(
    task: &str,
    store: &air_linker::ModuleStore,
    catalog: Vec<Value>,
    recipes: Vec<Value>,
    allow_internal: bool,
) -> Value {
    let component_selection = planner_component_selection(task, &catalog, &recipes);
    json!({
        "task": task.trim(),
        "module_store": {
            "name": &store.store.name,
            "version": &store.store.version,
            "catalog_policy": {
                "default_visibility": if allow_internal { "public_and_internal" } else { "public_only" },
                "selection_rule": "Follow component_selection.first_choice when it covers the task. Prefer recipes, then public composite modules, then bounded dynamic fan-out with reusable modules. Use primitive/internal modules only as an explicit fallback when no larger component satisfies the requested input/output contract."
            },
            "modules": catalog,
            "recipes": recipes,
            "component_selection": component_selection,
        },
        "instructions": planner_instructions(),
        "example_shape": {
            "plan": {"name": "task-run-plan", "version": "0.1.0"},
            "requires": {"capabilities": []},
            "nodes": [{"id": "extract", "module": "customer.extract@0.1.0"}],
            "entry": "extract",
            "edges": [],
            "connect": [{"from": "$input.text", "to": "extract.text"}],
            "outputs": {"result": "extract.extracted"},
            "halts": [],
            "schedule": {"max_parallel": 4, "groups": [{"id": "research_wave", "nodes": ["topic_1", "topic_2"], "max_parallel": 2}]},
            "decisions": [{"id": "choose-modules", "rationale": "why these modules were selected", "selected": ["extract"]}]
        },
        "example_dynamic_fanout_shape": {
            "nodes": [
                {"id": "plan", "module": "deep_research.plan_array@0.1.0"},
                {"id": "final", "module": "deep_research.final_report@0.1.0"}
            ],
            "requires": {"capabilities": ["network.search", "research.reflect"]},
            "entry": "plan",
            "edges": [{"from": "plan", "to": "final"}],
            "connect": [
                {"from": "$input.question", "to": "plan.question"},
                {"from": "$input.question", "to": "final.question"},
                {"from": "plan.plan", "to": "final.plan"}
            ],
            "dynamic": {
                "fanouts": [{
                    "id": "topic_wave",
                    "after": "plan",
                    "source": "plan.plan.topics",
                    "module": "deep_research.research_topic@0.1.0",
                    "id_prefix": "topic",
                    "min_items": 1,
                    "max_items": 4,
                    "max_parallel": 4,
                    "input": [
                        {"from": "plan.plan.research_brief", "to": "$each.research_brief"},
                        {"from": "$item", "to": "$each.topic"}
                    ],
                    "fan_in": [
                        {"from": "$each.note", "to": "final.notes[]"}
                    ]
                }]
            },
            "outputs": {"result": "final.final_report"}
        },
        "example_nested_dynamic_fanout_shape": {
            "nodes": [
                {"id": "plan", "module": "planner.with_topics@0.1.0"},
                {"id": "final", "module": "report.with_notes@0.1.0"}
            ],
            "requires": {"capabilities": ["network.search"]},
            "entry": "plan",
            "edges": [{"from": "plan", "to": "final"}],
            "dynamic": {
                "fanouts": [
                    {
                        "id": "parent_wave",
                        "after": "plan",
                        "source": "plan.topics",
                        "module": "research.parent@0.1.0",
                        "id_prefix": "parent",
                        "min_items": 1,
                        "max_items": 4,
                        "input": [
                            {"from": "$item", "to": "$each.topic"}
                        ],
                        "fan_in": [
                            {"from": "$each.note", "to": "final.notes[]"}
                        ]
                    },
                    {
                        "id": "child_wave",
                        "after": "parent_wave",
                        "source": "$parent.followups",
                        "module": "research.child@0.1.0",
                        "id_prefix": "child",
                        "min_items": 0,
                        "max_items": 3,
                        "input": [
                            {"from": "$item", "to": "$each.topic"}
                        ],
                        "fan_in": [
                            {"from": "$each.note", "to": "final.notes[]"}
                        ]
                    }
                ]
            },
            "outputs": {"result": "final.report"}
        },
        "example_recipe_selection": {
            "recipe_id": "deep_research.four_topic_report@0.1.0",
            "rationale": "The task asks for a bounded four-topic deep research report, which this recipe covers."
        },
        "example_repeated_module_shape": {
            "nodes": [
                {"id": "plan", "module": "deep_research.plan@0.1.0"},
                {"id": "topic_1", "module": "deep_research.research_topic@0.1.0"},
                {"id": "topic_2", "module": "deep_research.research_topic@0.1.0"},
                {"id": "final", "module": "deep_research.final_report@0.1.0"}
            ],
            "connect": [
                {"from": "$input.question", "to": "plan.question"},
                {"from": "plan.plan.research_brief", "to": "topic_1.research_brief"},
                {"from": "plan.plan.topic_1", "to": "topic_1.topic"},
                {"from": "topic_1.note", "to": "final.notes[]"},
                {"from": "topic_2.note", "to": "final.notes[]"}
            ]
        }
    })
}

fn planner_instructions() -> Vec<&'static str> {
    vec![
        "Return ONLY JSON.",
        "Read module_store.component_selection before module_store.modules. If component_selection.first_choice covers the task, select it.",
        "If module_store.recipes contains a recipe that covers the task, return a recipe selection object: {\"recipe_id\":\"...\",\"rationale\":\"...\"}. Prefer this over copying the full topology.",
        "If you reject component_selection.first_choice, include a decision with id reject-first-choice explaining the missing input/output or capability requirement.",
        "Otherwise return a full AIR RunPlan object with keys: plan, requires, nodes, entry, edges, connect, outputs, decisions. It may include halts when the plan should stop early on a typed condition.",
        "Set requires.capabilities to the union of all selected module requires.capabilities, including modules used by dynamic.fanouts.",
        "When independent fan-out nodes can run as a bounded parallel wave, include schedule.groups with a stable group id, node ids, and max_parallel.",
        "Use dynamic.fanouts when a module output is a typed array whose runtime length decides how many repeated module instances to create.",
        "For dynamic.fanouts, declare id, after, source, module, id_prefix, max_items, optional min_items, optional max_parallel, input, and fan_in.",
        "In dynamic.fanouts input mappings, use $item for the current source array item and $each.<input> for the materialized module input.",
        "In dynamic.fanouts fan_in mappings, use $each.<output> as the materialized module output and append to an array input such as final.notes[].",
        "Do not pre-list dynamic fan-out instances in nodes, edges, or connect; AIR materializes them after the declared after node.",
        "For constrained nested dynamic fan-out, set after to the parent fanout id and source to $parent.<array_output>; AIR expands one child wave per materialized parent node.",
        "Use static repeated nodes or a recipe when the exact fan-out cardinality is known before execution.",
        "If you return a full RunPlan from a recipe, preserve the recipe topology unless the task requires a smaller or clearly different bounded graph.",
        "Use module identifiers exactly as provided in module_store.modules[].id.",
        "Prefer composite modules with visibility=public and higher priority when they cover the task.",
        "Do not decompose a task into primitive modules when one public composite module covers the same task.",
        "When using primitive/internal modules as fallback, include a decision with id fallback-to-primitives and a rationale naming the missing larger component.",
        "You may instantiate the same module id multiple times with different node ids when the task needs repeated bounded work.",
        "Every node id must be unique and must be used consistently in edges, connect, outputs, and decisions.",
        "edges must contain node ids only, for example {\"from\":\"extract\",\"to\":\"score\"}.",
        "connect must contain endpoint ids, for example {\"from\":\"extract.extracted\",\"to\":\"score.extracted\"}.",
        "connect sources may read nested output object paths, for example {\"from\":\"plan.plan.topic_1\",\"to\":\"topic_1.topic\"}.",
        "connect targets may append to array inputs using [] when the target input schema is an array, for example {\"from\":\"topic_1.note\",\"to\":\"final.notes[]\"}.",
        "Every required module input listed in the catalog must be covered by exactly one direct connection or one or more [] append connections.",
        "Connect $input fields to every module input that is not provided by an upstream module output, for example connect $input.question to every downstream question input.",
        "Connect upstream module outputs to downstream inputs only when the names and schemas make sense.",
        "Set outputs to the final user-facing module output.",
        "Use halts for clarification or approval gates, for example {\"after\":\"clarify\",\"when\":{\"ref\":\"clarify.clarification.needs_clarification\",\"equals\":true},\"outputs\":{\"clarification\":\"clarify.clarification\"}}.",
        "Do not include markdown, comments, or explanatory text.",
    ]
}

fn planner_component_selection(task: &str, catalog: &[Value], recipes: &[Value]) -> Value {
    let task_terms = tokenize_for_component_match(task);
    let mut candidates = Vec::new();

    for recipe in recipes {
        let id = recipe.get("id").and_then(Value::as_str).unwrap_or_default();
        let priority = recipe
            .get("priority")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let matched_terms = matched_component_terms(
            &task_terms,
            &[
                recipe.get("id"),
                recipe.get("description"),
                recipe.get("tags"),
                recipe.get("covers"),
            ],
        );
        let score = priority + 1_000 + (matched_terms.len() as i64 * 25);
        candidates.push(json!({
            "id": id,
            "source": "recipe",
            "tier": "large_component",
            "kind": "recipe",
            "priority": priority,
            "score": score,
            "matched_terms": matched_terms,
            "use_when": "Use directly when the task fits this verified topology; return {recipe_id, rationale} instead of copying nodes."
        }));
    }

    for module in catalog {
        let id = module.get("id").and_then(Value::as_str).unwrap_or_default();
        let kind = module
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("primitive");
        let visibility = module
            .get("visibility")
            .and_then(Value::as_str)
            .unwrap_or("public");
        let priority = module
            .get("priority")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let matched_terms = matched_component_terms(
            &task_terms,
            &[
                module.get("id"),
                module.get("description"),
                module.get("tags"),
                module.get("covers"),
                module
                    .get("agent")
                    .and_then(|agent| agent.get("description")),
            ],
        );
        let tier = match (kind, visibility) {
            ("composite", "public") => "large_component",
            ("composite", _) => "internal_composite",
            ("primitive", "public") => "public_building_block",
            _ => "internal_building_block",
        };
        let tier_bonus = match tier {
            "large_component" => 500,
            "internal_composite" => 250,
            "public_building_block" => 50,
            _ => -100,
        };
        let score = priority + tier_bonus + (matched_terms.len() as i64 * 20);
        candidates.push(json!({
            "id": id,
            "source": "module",
            "tier": tier,
            "kind": kind,
            "visibility": visibility,
            "priority": priority,
            "score": score,
            "matched_terms": matched_terms,
            "use_when": match tier {
                "large_component" => "Prefer when its inputs/outputs cover the task.",
                "internal_composite" => "Use only when internal modules are allowed and no public recipe/composite covers the task.",
                "public_building_block" => "Use as a small component when no larger component covers the task.",
                _ => "Use only as explicit fallback glue inside a bounded plan.",
            }
        }));
    }

    candidates.sort_by(|left, right| {
        let left_score = left
            .get("score")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let right_score = right
            .get("score")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        right_score.cmp(&left_score).then_with(|| {
            left.get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .cmp(right.get("id").and_then(Value::as_str).unwrap_or_default())
        })
    });

    let first_choice = candidates.first().cloned().unwrap_or(Value::Null);
    let recommended = candidates.into_iter().take(8).collect::<Vec<_>>();
    json!({
        "selection_ladder": [
            "1. recipe: pre-validated topology, select with recipe_id when it covers the task",
            "2. public composite module: one larger AIR module that covers the task",
            "3. bounded dynamic fan-out: planner/composite plus repeated reusable module when cardinality is runtime-known",
            "4. primitive/internal modules: fallback only, with explicit rationale"
        ],
        "first_choice": first_choice,
        "recommended": recommended,
        "fallback_requirement": "If using lower-tier components while a higher-tier candidate is present, explain the missing contract in decisions."
    })
}

fn tokenize_for_component_match(value: &str) -> BTreeSet<String> {
    value
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .filter_map(normalize_component_token)
        .collect()
}

fn matched_component_terms(
    task_terms: &BTreeSet<String>,
    values: &[Option<&Value>],
) -> Vec<String> {
    let mut matched = BTreeSet::new();
    for value in values.iter().flatten() {
        collect_matched_component_terms(task_terms, value, &mut matched);
    }
    matched.into_iter().collect()
}

fn collect_matched_component_terms(
    task_terms: &BTreeSet<String>,
    value: &Value,
    matched: &mut BTreeSet<String>,
) {
    match value {
        Value::String(text) => {
            for token in text
                .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
                .filter_map(normalize_component_token)
            {
                if task_terms.contains(&token) {
                    matched.insert(token);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_matched_component_terms(task_terms, value, matched);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_matched_component_terms(task_terms, value, matched);
            }
        }
        _ => {}
    }
}

fn normalize_component_token(value: &str) -> Option<String> {
    let token = value.trim().to_ascii_lowercase();
    if token.len() < 3 || COMPONENT_STOP_WORDS.contains(&token.as_str()) {
        None
    } else {
        Some(token)
    }
}

const COMPONENT_STOP_WORDS: &[&str] = &[
    "and", "are", "for", "from", "into", "one", "the", "this", "that", "with", "when",
];

pub(crate) fn module_catalog(
    store: &air_linker::ModuleStore,
    base_dir: &std::path::Path,
    allow_internal: bool,
) -> Result<Vec<Value>> {
    let mut catalog = Vec::new();

    for (id, module_ref) in &store.modules {
        if !allow_internal && module_ref.visibility == air_linker::ModuleVisibility::Internal {
            continue;
        }

        let path = air_linker::resolve_module_path(base_dir, id, &module_ref.path)?;
        let module = air_parser::parse_air_file(&path)?;
        let report = air_verify::verify(&module);
        if !report.is_success() {
            anyhow::bail!(
                "store module {id} failed verification: {:?}",
                report.diagnostics
            );
        }

        catalog.push(json!({
            "id": id,
            "kind": module_ref.kind,
            "visibility": module_ref.visibility,
            "description": module_ref.description,
            "tags": module_ref.tags,
            "covers": module_ref.covers,
            "priority": module_ref.priority,
            "path": module_ref.path,
            "agent": {
                "name": module.agent.name,
                "version": module.agent.version,
                "description": module.agent.description,
            },
            "inputs": module.inputs,
            "outputs": module.outputs,
            "requires": module.requires,
            "tools": module.tools,
        }));
    }

    catalog.sort_by(|left, right| {
        let left_priority = left
            .get("priority")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let right_priority = right
            .get("priority")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        right_priority.cmp(&left_priority).then_with(|| {
            left.get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .cmp(right.get("id").and_then(Value::as_str).unwrap_or_default())
        })
    });

    Ok(catalog)
}

pub(crate) fn module_base_dir_for_store_path(
    store: &air_linker::ModuleStore,
    store_path: &Path,
) -> PathBuf {
    let start_dir = store_path.parent().unwrap_or_else(|| Path::new("."));
    let start_dir = start_dir
        .canonicalize()
        .unwrap_or_else(|_| start_dir.to_path_buf());
    infer_module_base_dir(&start_dir, &store.modules)
}

pub(crate) fn module_base_dir_for_modules(
    modules: &BTreeMap<String, air_linker::ModuleRef>,
) -> Result<PathBuf> {
    let current_dir = std::env::current_dir()?;
    Ok(infer_module_base_dir(&current_dir, modules))
}

fn infer_module_base_dir(
    start_dir: &Path,
    modules: &BTreeMap<String, air_linker::ModuleRef>,
) -> PathBuf {
    if modules.values().all(|module| module.path.is_absolute()) {
        return start_dir.to_path_buf();
    }

    start_dir
        .ancestors()
        .find(|candidate| {
            modules
                .values()
                .filter(|module| !module.path.is_absolute())
                .all(|module| candidate.join(&module.path).exists())
        })
        .unwrap_or(start_dir)
        .to_path_buf()
}

pub(crate) fn recipe_catalog(
    store: &air_linker::ModuleStore,
    base_dir: &std::path::Path,
    allow_internal: bool,
) -> Result<Vec<Value>> {
    let mut recipes = Vec::new();

    for recipe in &store.recipes {
        if !allow_internal && recipe_uses_internal_modules(recipe, store) {
            continue;
        }

        let report = air_linker::validate_run_plan(&recipe.plan, store, base_dir);
        if !report.is_success() {
            anyhow::bail!(
                "store recipe {} failed verification: {:?}",
                recipe.id,
                report.diagnostics
            );
        }

        recipes.push(json!({
            "id": recipe.id,
            "description": recipe.description,
            "tags": recipe.tags,
            "covers": recipe.covers,
            "priority": recipe.priority,
            "topology": {
                "nodes": recipe.plan.nodes.iter().map(|node| {
                    json!({
                        "id": node.id,
                        "module": node.module,
                        "when": node.when,
                    })
                }).collect::<Vec<_>>(),
                "entry": &recipe.plan.entry,
                "edges": recipe.plan.edges.iter().map(|edge| {
                    format!("{} -> {}", edge.from, edge.to)
                }).collect::<Vec<_>>(),
                "connect": recipe.plan.connect.iter().map(|connection| {
                    format!("{} -> {}", display_connection_source(connection), connection.to)
                }).collect::<Vec<_>>(),
                "outputs": &recipe.plan.outputs,
                "requires": &recipe.plan.requires,
            },
        }));
    }

    recipes.sort_by(|left, right| {
        let left_priority = left
            .get("priority")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let right_priority = right
            .get("priority")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        right_priority.cmp(&left_priority).then_with(|| {
            left.get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .cmp(right.get("id").and_then(Value::as_str).unwrap_or_default())
        })
    });

    Ok(recipes)
}

pub(crate) fn parse_planner_response(
    value: Value,
    store: &air_linker::ModuleStore,
    base_dir: &std::path::Path,
    allow_internal: bool,
) -> Result<air_linker::RunPlan> {
    let value = planner_json_value(value)?;
    if let Ok(plan) = serde_json::from_value::<air_linker::RunPlan>(value.clone()) {
        return Ok(plan);
    }

    if let Some(recipe_id) = value
        .get("recipe_id")
        .or_else(|| value.get("recipe"))
        .and_then(Value::as_str)
    {
        return materialize_recipe_selection(recipe_id, &value, store, base_dir, allow_internal);
    }

    anyhow::bail!("planner response was neither a RunPlan nor a recipe selection object")
}

fn planner_json_value(value: Value) -> Result<Value> {
    let Some(content) = value.get("content").and_then(Value::as_str) else {
        return Ok(value);
    };
    let content = strip_json_fence(content);
    Ok(serde_json::from_str(content)?)
}

fn materialize_recipe_selection(
    recipe_id: &str,
    selection: &Value,
    store: &air_linker::ModuleStore,
    base_dir: &std::path::Path,
    allow_internal: bool,
) -> Result<air_linker::RunPlan> {
    let recipe = store
        .recipes
        .iter()
        .find(|recipe| recipe.id == recipe_id)
        .ok_or_else(|| anyhow::anyhow!("planner selected unknown recipe {recipe_id}"))?;

    if !allow_internal && recipe_uses_internal_modules(recipe, store) {
        anyhow::bail!("planner selected internal recipe {recipe_id} without --allow-internal");
    }

    let report = air_linker::validate_run_plan(&recipe.plan, store, base_dir);
    if !report.is_success() {
        anyhow::bail!(
            "planner selected invalid recipe {recipe_id}: {:?}",
            report.diagnostics
        );
    }

    let mut plan = recipe.plan.clone();
    let rationale = selection
        .get("rationale")
        .and_then(Value::as_str)
        .unwrap_or("Planner selected a module-store recipe.");
    plan.decisions.insert(
        0,
        air_linker::PlanDecision {
            id: "select-recipe".to_string(),
            rationale: format!("Selected recipe {recipe_id}: {rationale}"),
            selected: vec![recipe_id.to_string()],
        },
    );
    Ok(plan)
}

fn recipe_uses_internal_modules(
    recipe: &air_linker::PlanRecipe,
    store: &air_linker::ModuleStore,
) -> bool {
    recipe.plan.nodes.iter().any(|node| {
        store.modules.get(&node.module).is_some_and(|module_ref| {
            module_ref.visibility == air_linker::ModuleVisibility::Internal
        })
    })
}

fn normalize_planner_run_plan(plan: &mut air_linker::RunPlan) {
    let mut normalized_edges = Vec::new();
    let mut inferred_connections = Vec::new();

    for edge in &plan.edges {
        let from_endpoint = endpoint_node(&edge.from);
        let to_endpoint = endpoint_node(&edge.to);
        if let (Some(from_node), Some(to_node)) = (from_endpoint, to_endpoint) {
            inferred_connections.push(air_linker::Connection {
                from: Some(edge.from.clone()),
                to: edge.to.clone(),
                value: None,
            });
            normalized_edges.push(air_linker::SystemEdge {
                from: from_node.to_string(),
                to: to_node.to_string(),
            });
        } else {
            normalized_edges.push(edge.clone());
        }
    }

    plan.edges = dedupe_edges(normalized_edges);
    plan.connect = dedupe_connections(
        plan.connect
            .iter()
            .cloned()
            .chain(inferred_connections)
            .collect(),
    );
}

fn endpoint_node(endpoint: &str) -> Option<&str> {
    let (node, field) = endpoint.split_once('.')?;
    (!node.is_empty() && !field.is_empty() && !field.contains('.')).then_some(node)
}

fn dedupe_edges(edges: Vec<air_linker::SystemEdge>) -> Vec<air_linker::SystemEdge> {
    let mut deduped = Vec::new();
    for edge in edges {
        if !deduped.iter().any(|existing: &air_linker::SystemEdge| {
            existing.from == edge.from && existing.to == edge.to
        }) {
            deduped.push(edge);
        }
    }
    deduped
}

fn dedupe_connections(connections: Vec<air_linker::Connection>) -> Vec<air_linker::Connection> {
    let mut deduped = Vec::new();
    for connection in connections {
        if !deduped.iter().any(|existing: &air_linker::Connection| {
            existing.from == connection.from
                && existing.value == connection.value
                && existing.to == connection.to
        }) {
            deduped.push(connection);
        }
    }
    deduped
}

fn display_connection_source(connection: &air_linker::Connection) -> String {
    if let Some(from) = &connection.from {
        return from.clone();
    }
    if let Some(value) = &connection.value {
        return serde_json::to_string(value).unwrap_or_else(|_| "<value>".to_string());
    }
    "<missing>".to_string()
}

fn strip_json_fence(content: &str) -> &str {
    let trimmed = content.trim();
    let Some(after_open) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let Some(close_index) = after_open.rfind("```") else {
        return trimmed;
    };
    let inner = &after_open[..close_index];
    let inner = inner.trim_start();
    let inner = inner
        .strip_prefix("json")
        .or_else(|| inner.strip_prefix("JSON"))
        .unwrap_or(inner);
    inner.trim()
}
