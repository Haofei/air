use air_advisory::{
    cached_llm_advisory, is_disabled as llm_advisory_disabled, CachedLlmAdvisoryOptions,
};
use air_code_artifact::{path_content_identity, CodeRunSkill, CodeRunVerdictConstraints};
use air_runtime::ModelProvider;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const SKILL_MANIFEST: &str = "air-skill.yaml";
const SKILL_SCHEMA: &str = air_schemas::SKILL;
const DEFAULT_EXECUTOR_SKILL: &str = "code-agent";
const SKILL_ROUTER_MODEL: &str = "skill_router";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirSkillManifest {
    #[serde(default)]
    pub schema: Option<String>,
    pub id: String,
    #[serde(default)]
    pub mode: SkillMode,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub source: Option<Value>,
    #[serde(default)]
    pub instructions: SkillInstructions,
    #[serde(default)]
    pub capabilities: SkillCapabilities,
    #[serde(default)]
    pub tools: SkillTools,
    #[serde(default)]
    pub host: Option<SkillHost>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<SkillWorkflow>,
    #[serde(default)]
    pub routing: SkillRouting,
    #[serde(default)]
    pub verification: Value,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillMode {
    Instruction,
    #[default]
    Executor,
}

impl SkillMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Instruction => "instruction",
            Self::Executor => "executor",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillInstructions {
    #[serde(default)]
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillCapabilities {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillTools {
    #[serde(default)]
    pub config: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillHost {
    pub skill: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillWorkflow {
    pub profile: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillRouting {
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub triggers: Vec<String>,
    #[serde(default)]
    pub negative_triggers: Vec<String>,
    #[serde(default)]
    pub repo_markers: Vec<PathBuf>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub task_types: Vec<String>,
    #[serde(default)]
    pub compatible_with: Vec<String>,
    #[serde(default)]
    pub incompatible_with: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedSkill {
    pub manifest_path: PathBuf,
    pub manifest_dir: PathBuf,
    pub manifest: AirSkillManifest,
}

pub struct SkillRouteOptions {
    pub task: String,
    pub top_k: usize,
    pub explain: bool,
    pub memory_pack: Option<Value>,
    pub advisory_provider: Option<Box<dyn ModelProvider>>,
}

pub struct SkillUpgradeOptions {
    pub skill: String,
    pub dry_run: bool,
}

#[derive(Debug, Deserialize)]
struct NativeLoopProfile {
    #[serde(default)]
    plan: Option<PathBuf>,
    #[serde(default)]
    store: Option<PathBuf>,
    #[serde(default)]
    tool_config: Option<PathBuf>,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    native_loop: Option<String>,
}

#[derive(Debug)]
struct CodeExplainMetadata {
    plan: Option<PathBuf>,
    store: Option<PathBuf>,
    native_loop: Option<String>,
    capabilities: Vec<String>,
    read_only: bool,
    writes_workspace: bool,
}

pub struct PreparedSkillRun {
    pub metadata: CodeRunSkill,
    pub profile: PathBuf,
    pub tool_config: Option<PathBuf>,
    pub task_prefix: Option<String>,
    pub artifact_extra: BTreeMap<String, Value>,
}

#[derive(Debug, Serialize)]
struct SkillListItem {
    id: String,
    mode: String,
    version: Option<String>,
    description: Option<String>,
    manifest: String,
    profile: String,
    tool_config: Option<String>,
    host: Option<String>,
    explain: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SkillRouteResult {
    executor: SkillRouteExecutor,
    instructions: Vec<SkillRouteCard>,
    selected: Vec<SkillRouteCard>,
    rejected: Vec<SkillRouteRejection>,
    candidates: Vec<SkillRouteCard>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    advisory: Option<SkillRouteAdvisory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    advisory_status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SkillRouteAdvisory {
    model: String,
    ranked_ids: Vec<String>,
    reason: String,
    confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SkillRouteExecutor {
    id: String,
    reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SkillRouteCard {
    id: String,
    host: String,
    mode: String,
    version: Option<String>,
    description: Option<String>,
    manifest: String,
    audit_risk: String,
    score: i32,
    reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SkillRouteRejection {
    id: String,
    manifest: String,
    reason: String,
}

#[derive(Debug, Default, Clone, Serialize)]
struct SkillFrontmatter {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    triggers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    repo_markers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    languages: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    task_types: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    allowed_tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillAudit {
    pub schema: String,
    pub skill_id: String,
    pub risk: String,
    #[serde(default)]
    pub trusted_source: bool,
    pub allowed_to_run: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_reason: Option<String>,
    pub findings: Vec<SkillAuditFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillAuditFinding {
    pub severity: String,
    pub reason: String,
    pub file: String,
}

pub fn list_skills_value() -> Result<Value> {
    let skills = discover_skills()?
        .into_iter()
        .map(|skill| skill.list_item())
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({ "skills": skills }))
}

pub fn route_skill_value(options: SkillRouteOptions) -> Result<Value> {
    let route = route_skills_for_task_advisory(
        &options.task,
        options.top_k.max(1),
        options.advisory_provider,
    )?;
    if options.explain {
        Ok(json!({
            "task": options.task,
            "executor": route.executor,
            "instructions": route.instructions,
            "selected": route.selected,
            "rejected": route.rejected,
            "candidates": route.candidates,
            "advisory": route.advisory,
            "advisory_status": route.advisory_status,
            "memory_pack": options.memory_pack,
        }))
    } else {
        Ok(json!({
            "task": options.task,
            "executor": route.executor,
            "instructions": route.instructions,
            "selected": route.selected,
            "advisory": route.advisory,
            "advisory_status": route.advisory_status,
            "memory_pack": options.memory_pack,
        }))
    }
}

fn route_skills_for_task(task: &str, top_k: usize) -> Result<SkillRouteResult> {
    if top_k == 0 {
        return Ok(SkillRouteResult {
            executor: SkillRouteExecutor {
                id: "code-agent".to_string(),
                reason: "skill routing disabled by top_k=0".to_string(),
            },
            instructions: Vec::new(),
            selected: Vec::new(),
            rejected: Vec::new(),
            candidates: Vec::new(),
            advisory: None,
            advisory_status: Some("not_needed_top_k_zero".to_string()),
        });
    }
    let root = if Path::new("skills").exists() {
        PathBuf::from(".")
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    };
    let route = air_skill_runtime::route_skills_for_task(&root, task, top_k)?;
    Ok(serde_json::from_value(serde_json::to_value(route)?)?)
}

fn route_skills_for_task_advisory(
    task: &str,
    top_k: usize,
    advisory_provider: Option<Box<dyn ModelProvider>>,
) -> Result<SkillRouteResult> {
    let mut route = route_skills_for_task(task, top_k)?;
    if route.candidates.is_empty() {
        route.advisory_status = Some("not_needed_no_candidates".to_string());
        return Ok(route);
    }
    let Some(mut provider) = advisory_provider else {
        route.advisory_status = Some("lexical_only_needs_llm_model_config".to_string());
        return Ok(route);
    };
    if llm_advisory_disabled() {
        route.advisory_status = Some("lexical_only_llm_advisory_disabled".to_string());
        return Ok(route);
    }
    let request = json!({
        "schema": air_schemas::SKILL_ROUTER_REQUEST,
        "task": task,
        "top_k": top_k,
        "deterministic_selected": route.selected.iter().map(|card| card.id.as_str()).collect::<Vec<_>>(),
        "candidates": route.candidates.iter().map(|card| json!({
            "id": card.id,
            "mode": card.mode,
            "host": card.host,
            "description": card.description,
            "score": card.score,
            "reasons": card.reasons,
        })).collect::<Vec<_>>(),
        "instruction": "Rank only the listed candidate skill ids for this task. Do not invent skill ids.",
    });
    let candidate_ids = route
        .candidates
        .iter()
        .map(|card| card.id.clone())
        .collect::<BTreeSet<_>>();
    let advisory = cached_llm_advisory(
        CachedLlmAdvisoryOptions {
            cache_root: Path::new(".air/memory/cache"),
            out_file: None,
            purpose: "skill-router",
            model_alias: SKILL_ROUTER_MODEL,
            request: &request,
            generated_at_unix: unix_now(),
        },
        provider.as_mut(),
        |output| validate_skill_router_output(output, &candidate_ids),
    );
    let advisory = match advisory {
        Ok(advisory) => advisory,
        Err(error) => {
            route.advisory_status = Some(format!(
                "lexical_only_llm_advisory_failed: {}",
                truncate_for_status(&error.to_string(), 180)
            ));
            return Ok(route);
        }
    };
    let by_id = route
        .candidates
        .iter()
        .map(|card| (card.id.clone(), card.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut selected = Vec::new();
    let mut seen = BTreeSet::new();
    for id in &advisory.ranked_ids {
        if selected.len() >= top_k {
            break;
        }
        if seen.insert(id.clone()) {
            if let Some(card) = by_id.get(id) {
                selected.push(card.clone());
            }
        }
    }
    for card in &route.selected {
        if selected.len() >= top_k {
            break;
        }
        if seen.insert(card.id.clone()) {
            selected.push(card.clone());
        }
    }
    route.instructions = selected
        .iter()
        .filter(|card| card.mode == "instruction")
        .cloned()
        .collect();
    route.selected = selected;
    route.advisory = Some(advisory);
    route.advisory_status = Some("complete".to_string());
    Ok(route)
}

fn truncate_for_status(value: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for ch in value.chars().take(max_chars) {
        out.push(ch);
    }
    if value.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

#[derive(Debug, Deserialize)]
struct SkillRouterOutput {
    ranked_ids: Vec<String>,
    reason: String,
    #[serde(default)]
    confidence: Option<f64>,
}

fn validate_skill_router_output(
    output: Value,
    candidate_ids: &BTreeSet<String>,
) -> Result<SkillRouteAdvisory> {
    let parsed: SkillRouterOutput =
        serde_json::from_value(output).context("skill router output must be an object")?;
    let mut ranked_ids = Vec::new();
    let mut seen = BTreeSet::new();
    for id in parsed.ranked_ids {
        if candidate_ids.contains(&id) && seen.insert(id.clone()) {
            ranked_ids.push(id);
        }
    }
    if ranked_ids.is_empty() {
        anyhow::bail!("skill router did not return any known skill ids");
    }
    let reason = parsed.reason.trim().to_string();
    if reason.is_empty() {
        anyhow::bail!("skill router reason is empty");
    }
    Ok(SkillRouteAdvisory {
        model: SKILL_ROUTER_MODEL.to_string(),
        ranked_ids,
        reason,
        confidence: parsed.confidence.unwrap_or(0.5).clamp(0.0, 1.0),
    })
}

pub fn route_skills_value_for_task(task: &str, top_k: usize) -> Result<Value> {
    Ok(serde_json::to_value(route_skills_for_task(task, top_k)?)?)
}

fn route_tokens(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_ascii_alphanumeric())
        .map(str::trim)
        .filter(|token| token.len() >= 4)
        .map(str::to_ascii_lowercase)
        .collect()
}

fn skill_frontmatter_for_dir(skill_dir: &Path) -> Option<SkillFrontmatter> {
    for name in ["SKILL.md", "README.md"] {
        let path = skill_dir.join(name);
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        if let Some(frontmatter) = parse_skill_frontmatter(&content) {
            return Some(frontmatter);
        }
    }
    None
}

fn parse_skill_frontmatter(content: &str) -> Option<SkillFrontmatter> {
    let mut lines = content.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    let mut yaml = String::new();
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        yaml.push_str(line);
        yaml.push('\n');
    }
    if yaml.trim().is_empty() {
        return None;
    }
    let value: serde_yaml::Value = serde_yaml::from_str(&yaml).ok()?;
    Some(SkillFrontmatter {
        name: yaml_string_field(&value, &["name"]),
        title: yaml_string_field(&value, &["title"]),
        description: yaml_string_field(&value, &["description", "summary"]),
        triggers: yaml_string_list_field(&value, &["triggers", "keywords"]),
        repo_markers: yaml_string_list_field(&value, &["repo_markers", "repo-markers"]),
        languages: yaml_string_list_field(&value, &["languages"]),
        task_types: yaml_string_list_field(&value, &["task_types", "task-types"]),
        allowed_tools: yaml_string_list_field(&value, &["allowed_tools", "allowed-tools", "tools"]),
    })
}

fn yaml_string_field(value: &serde_yaml::Value, keys: &[&str]) -> Option<String> {
    let mapping = value.as_mapping()?;
    for key in keys {
        if let Some(value) = mapping.get(serde_yaml::Value::String((*key).to_string())) {
            if let Some(text) = value
                .as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
            {
                return Some(text.to_string());
            }
        }
    }
    None
}

fn yaml_string_list_field(value: &serde_yaml::Value, keys: &[&str]) -> Vec<String> {
    let Some(mapping) = value.as_mapping() else {
        return Vec::new();
    };
    for key in keys {
        let Some(value) = mapping.get(serde_yaml::Value::String((*key).to_string())) else {
            continue;
        };
        return value_to_string_list(value);
    }
    Vec::new()
}

fn value_to_string_list(value: &serde_yaml::Value) -> Vec<String> {
    match value {
        serde_yaml::Value::String(text) => split_skill_list_text(text),
        serde_yaml::Value::Sequence(items) => items
            .iter()
            .flat_map(value_to_string_list)
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    }
}

fn split_skill_list_text(text: &str) -> Vec<String> {
    text.split([',', '\n'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn validate_skill_value(reference: &str) -> Result<Value> {
    let skill = resolve_skill(reference)?;
    let checks = validate_resolved_skill(&skill)?;
    Ok(json!({
        "valid": true,
        "skill": skill.manifest.id,
        "manifest": path_ref(&skill.manifest_path),
        "checks": checks,
    }))
}

pub fn explain_skill_value(reference: &str, profile_override: Option<PathBuf>) -> Result<Value> {
    let skill = resolve_skill(reference)?;
    validate_resolved_skill(&skill)?;
    let effective = effective_execution(&skill)?;
    let profile = profile_override.unwrap_or_else(|| effective.profile.clone());
    let input = build_input("<task>".to_string(), None, true);
    let mut explanation = code_profile_explain(&skill.manifest.id, &profile, &input)?;
    if let Some(object) = explanation.as_object_mut() {
        object.insert(
            "mode".to_string(),
            Value::String(skill.manifest.mode.as_str().to_string()),
        );
        object.insert(
            "host".to_string(),
            effective
                .host_skill
                .clone()
                .map_or(Value::Null, Value::String),
        );
        object.insert(
            "effective_capabilities".to_string(),
            Value::Array(
                effective
                    .capabilities
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );
        object.insert(
            "manifest".to_string(),
            Value::String(path_ref(&skill.manifest_path)),
        );
        object.insert(
            "description".to_string(),
            skill
                .manifest
                .description
                .clone()
                .map_or(Value::Null, Value::String),
        );
        object.insert(
            "instructions".to_string(),
            Value::Array(
                skill
                    .manifest
                    .instructions
                    .files
                    .iter()
                    .map(|path| Value::String(path_ref(&skill.manifest_dir.join(path))))
                    .collect(),
            ),
        );
    }
    Ok(explanation)
}

pub fn verdict_constraints_for_executor(executor_id: &str) -> CodeRunVerdictConstraints {
    if matches!(executor_id, "review-agent" | "bench-agent") {
        CodeRunVerdictConstraints::review()
    } else {
        CodeRunVerdictConstraints::code_edit()
    }
}

pub fn prepare_skill_composition(instruction_references: &[String]) -> Result<PreparedSkillRun> {
    prepare_skill_composition_for_executor(DEFAULT_EXECUTOR_SKILL, instruction_references)
}

pub fn prepare_skill_composition_for_executor(
    executor_id: &str,
    instruction_references: &[String],
) -> Result<PreparedSkillRun> {
    let executor = resolve_skill(executor_id)?;
    validate_resolved_skill(&executor)?;
    let executor_audit = audit_resolved_skill(&executor)?;
    if !executor_audit.allowed_to_run {
        bail!(
            "executor skill `{}` audit risk is {}; refusing to run",
            executor.manifest.id,
            executor_audit.risk
        );
    }
    let effective = effective_execution(&executor)?;
    let metadata = skill_metadata_with_context(&executor, Some(&executor_audit), &effective)?;
    let mut preloaded_skills = Vec::new();
    let mut instruction_metadata = Vec::new();
    let mut seen = BTreeSet::new();
    for reference in instruction_references {
        if !seen.insert(reference.clone()) {
            continue;
        }
        let skill = resolve_skill(reference)?;
        validate_resolved_skill(&skill)?;
        if skill.manifest.mode != SkillMode::Instruction {
            bail!(
                "skill composition only accepts instruction skills; `{}` is {}",
                skill.manifest.id,
                skill.manifest.mode.as_str()
            );
        }
        let host = instruction_host_id(&skill.manifest);
        if host != executor_id {
            bail!(
                "instruction skill `{}` is hosted by `{host}`, but this composition uses `{executor_id}`",
                skill.manifest.id,
            );
        }
        let audit = audit_resolved_skill(&skill)?;
        if !audit.allowed_to_run {
            bail!(
                "instruction skill `{}` audit risk is {}; refusing to load",
                skill.manifest.id,
                audit.risk
            );
        }
        let instruction_effective = effective_execution(&skill)?;
        preloaded_skills.extend(preload_skill_for_run(&skill, &audit)?);
        instruction_metadata.push(skill_metadata_with_context(
            &skill,
            Some(&audit),
            &instruction_effective,
        )?);
    }
    let task_prefix = if preloaded_skills.is_empty() {
        None
    } else {
        Some(render_preloaded_skill_instructions(&preloaded_skills))
    };
    let artifact_extra = BTreeMap::from([
        ("skill".to_string(), json!(metadata.clone())),
        (
            "skill_composition".to_string(),
            json!({
                "executor": metadata.clone(),
                "instructions": instruction_metadata,
                "profile": path_ref(&effective.profile),
                "tool_config": effective.tool_config.as_ref().map(|path| path_ref(path)),
                "capabilities": effective.capabilities,
                "preloaded_instruction_files": preloaded_instruction_files(&preloaded_skills),
            }),
        ),
    ]);
    Ok(PreparedSkillRun {
        metadata,
        profile: effective.profile,
        tool_config: effective.tool_config,
        task_prefix,
        artifact_extra,
    })
}

pub fn audit_skill_value(reference: &str, write: bool) -> Result<Value> {
    let skill = resolve_skill(reference)?;
    let audit = audit_resolved_skill(&skill)?;
    if write {
        let path = skill.manifest_dir.join("audit.json");
        fs::write(&path, serde_json::to_vec_pretty(&audit)?)
            .with_context(|| format!("write {}", path.display()))?;
    }
    Ok(json!(audit))
}

pub fn upgrade_skill(options: SkillUpgradeOptions) -> Result<()> {
    if !options.dry_run {
        bail!("skill upgrade currently supports only --dry-run");
    }
    let current = resolve_skill(&options.skill)?;
    let source_path = current.manifest_dir.join("source.json");
    let source_metadata: Value = serde_json::from_slice(
        &fs::read(&source_path).with_context(|| format!("read {}", source_path.display()))?,
    )
    .with_context(|| format!("parse {}", source_path.display()))?;
    let source = source_metadata
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("skill source.json missing string field `source`"))?;

    let temp_dir = std::env::temp_dir().join(format!(
        "air-skill-upgrade-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir_all(&temp_dir).with_context(|| format!("create {}", temp_dir.display()))?;
    let imported_root = import_source_to_temp(source, &temp_dir)?;
    let import_units = discover_import_units(&imported_root)?;
    let unit = import_units
        .iter()
        .find(|unit| unit.id == current.manifest.id)
        .or_else(|| {
            if import_units.len() == 1 {
                import_units.first()
            } else {
                None
            }
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "imported source did not contain a matching skill id `{}`",
                current.manifest.id
            )
        })?;
    let candidate_dir = temp_dir.join("candidate").join(&current.manifest.id);
    copy_dir_all(&unit.path, &candidate_dir)?;
    let source_label = if unit.relative.as_os_str().is_empty() {
        source.to_string()
    } else {
        format!("{source}#{}", path_ref(&unit.relative))
    };
    ensure_import_manifest(&source_label, &candidate_dir)?;
    let candidate = resolve_skill(candidate_dir.to_string_lossy().as_ref())?;
    let current_audit = audit_resolved_skill(&current)?;
    let candidate_audit = audit_resolved_skill(&candidate)?;
    let current_files = skill_file_identity_map(&current.manifest_dir)?;
    let candidate_files = skill_file_identity_map(&candidate.manifest_dir)?;
    let diff = skill_file_diff(&current_files, &candidate_files);
    let files_changed = skill_file_diff_changed(&diff);
    let current_sha256 = directory_identity(&current.manifest_dir)?;
    let candidate_sha256 = directory_identity(&candidate.manifest_dir)?;
    let current_capabilities = skill_capability_set(&current.manifest);
    let candidate_capabilities = skill_capability_set(&candidate.manifest);
    let capability_added = candidate_capabilities
        .difference(&current_capabilities)
        .cloned()
        .collect::<Vec<_>>();
    let capability_removed = current_capabilities
        .difference(&candidate_capabilities)
        .cloned()
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(temp_dir);
    print_json(&json!({
        "dry_run": true,
        "skill": current.manifest.id,
        "source": source,
        "current_sha256": current_sha256,
        "candidate_sha256": candidate_sha256,
        "changed": files_changed
            || !capability_added.is_empty()
            || !capability_removed.is_empty(),
        "files": diff,
        "capabilities": {
            "added": capability_added,
            "removed": capability_removed,
        },
        "audit": {
            "current": current_audit,
            "candidate": candidate_audit,
        },
    }))
}

pub fn import_skill_value(source: &str, out: Option<&Path>) -> Result<Value> {
    let temp_dir = std::env::temp_dir().join(format!(
        "air-skill-import-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir_all(&temp_dir).with_context(|| format!("create {}", temp_dir.display()))?;
    let imported_root = import_source_to_temp(source, &temp_dir)?;
    let import_units = discover_import_units(&imported_root)?;
    let imported = if import_units.len() == 1 {
        let unit = &import_units[0];
        let destination = out
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("skills/vendor").join(&unit.id));
        if destination.exists() {
            bail!(
                "skill import output already exists: {}",
                destination.display()
            );
        }
        vec![copy_import_unit(
            source,
            &imported_root,
            unit,
            &destination,
        )?]
    } else {
        let destination_root = out
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("skills/vendor"));
        fs::create_dir_all(&destination_root)
            .with_context(|| format!("create {}", destination_root.display()))?;
        let mut results = Vec::new();
        for unit in &import_units {
            let destination = destination_root.join(&unit.id);
            if destination.exists() {
                results.push(json!({
                    "skill": unit.id,
                    "source_path": path_ref(&unit.relative),
                    "skipped": true,
                    "reason": "destination exists",
                    "destination": path_ref(&destination),
                }));
                continue;
            }
            results.push(copy_import_unit(
                source,
                &imported_root,
                unit,
                &destination,
            )?);
        }
        results
    };
    let _ = fs::remove_dir_all(temp_dir);
    Ok(json!({
        "source": source,
        "count": imported.len(),
        "imported": imported,
    }))
}

pub fn resolve_skill(reference: &str) -> Result<ResolvedSkill> {
    let path = PathBuf::from(reference);
    let manifest_path = if path.exists() {
        manifest_path_for_existing_path(&path)?
    } else {
        find_skill_manifest_by_id(reference)
            .with_context(|| format!("unknown AIR skill `{reference}`; run `air skill list`"))?
    };
    read_skill_manifest(&manifest_path)
}

#[derive(Debug, Clone)]
struct ImportUnit {
    id: String,
    path: PathBuf,
    relative: PathBuf,
}

fn discover_import_units(root: &Path) -> Result<Vec<ImportUnit>> {
    let dirs = discover_import_skill_dirs(root)?;
    if dirs.is_empty() {
        bail!(
            "skill import source does not contain a SKILL.md or importable skill collection: {}",
            root.display()
        );
    }
    let mut ids = BTreeSet::new();
    let mut units = Vec::new();
    for dir in dirs {
        let relative = dir.strip_prefix(root).unwrap_or(&dir).to_path_buf();
        let base_id = imported_skill_id(&dir);
        let mut id = base_id.clone();
        let mut suffix = 2usize;
        while ids.contains(&id) {
            id = format!("{base_id}-{suffix}");
            suffix += 1;
        }
        ids.insert(id.clone());
        units.push(ImportUnit {
            id,
            path: dir,
            relative,
        });
    }
    units.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(units)
}

fn discover_import_skill_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    if root.join(SKILL_MANIFEST).exists() || root.join("SKILL.md").exists() {
        return Ok(vec![root.to_path_buf()]);
    }
    for collection in [
        "skills",
        ".claude/skills",
        ".agents/skills",
        ".opencode/skills",
    ] {
        let path = root.join(collection);
        let dirs = direct_skill_children(&path)?;
        if !dirs.is_empty() {
            return Ok(dirs);
        }
    }
    let dirs = direct_skill_children(root)?;
    if !dirs.is_empty() {
        return Ok(dirs);
    }
    Ok(Vec::new())
}

fn direct_skill_children(root: &Path) -> Result<Vec<PathBuf>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut dirs = Vec::new();
    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        if path.join("SKILL.md").exists() || path.join(SKILL_MANIFEST).exists() {
            dirs.push(path);
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn imported_skill_id(skill_dir: &Path) -> String {
    let frontmatter = skill_frontmatter_for_dir(skill_dir).unwrap_or_default();
    let raw = frontmatter.name.or(frontmatter.title).unwrap_or_else(|| {
        skill_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("imported-skill")
            .to_string()
    });
    slugify_skill_id(&raw)
}

fn slugify_skill_id(raw: &str) -> String {
    let mut out = String::new();
    let mut last_was_dash = false;
    for character in raw.chars() {
        if character.is_ascii_alphanumeric() {
            out.push(character.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash && !out.is_empty() {
            out.push('-');
            last_was_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "imported-skill".to_string()
    } else {
        out
    }
}

fn copy_import_unit(
    source: &str,
    _source_root: &Path,
    unit: &ImportUnit,
    destination: &Path,
) -> Result<Value> {
    fs::create_dir_all(destination.parent().unwrap_or_else(|| Path::new(".")))
        .with_context(|| format!("create parent for {}", destination.display()))?;
    copy_dir_all(&unit.path, destination)?;
    let source_label = if unit.relative.as_os_str().is_empty() {
        source.to_string()
    } else {
        format!("{source}#{}", path_ref(&unit.relative))
    };
    ensure_import_manifest(&source_label, destination)?;
    let resolved = resolve_skill(destination.to_string_lossy().as_ref())?;
    let trusted = is_trusted_import_source(source);
    let source_metadata = json!({
        "source": source,
        "source_path": path_ref(&unit.relative),
        "imported_at_unix_ms": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
        "manifest": path_ref(&resolved.manifest_path),
        "sha256": directory_identity(destination)?,
        "trusted": trusted,
        "trusted_by": if trusted {
            Some("air importer rule: github.com/anthropics/skills")
        } else {
            None
        },
    });
    fs::write(
        destination.join("source.json"),
        serde_json::to_vec_pretty(&source_metadata)?,
    )
    .with_context(|| format!("write {}", destination.join("source.json").display()))?;
    let audit = audit_resolved_skill(&resolved)?;
    fs::write(
        destination.join("audit.json"),
        serde_json::to_vec_pretty(&audit)?,
    )
    .with_context(|| format!("write {}", destination.join("audit.json").display()))?;
    update_skills_lock(source, destination, &resolved, &audit, trusted)?;
    Ok(json!({
        "imported": path_ref(destination),
        "skill": resolved.manifest.id,
        "source_path": path_ref(&unit.relative),
        "audit_risk": audit.risk,
        "allowed_to_run": audit.allowed_to_run,
    }))
}

fn update_skills_lock(
    source: &str,
    destination: &Path,
    resolved: &ResolvedSkill,
    audit: &SkillAudit,
    trusted: bool,
) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let lock_path = cwd.join(air_skill_runtime::SKILLS_LOCK);
    let mut lock = air_skill_runtime::read_skills_lock(&lock_path)?;
    let audit_path = destination.join("audit.json");
    lock.skills.insert(
        resolved.manifest.id.clone(),
        air_skill_runtime::SkillLockEntry {
            manifest: path_ref(&resolved.manifest_path),
            source: Some(source.to_string()),
            sha256: directory_identity(destination)?,
            audit_sha256: if audit_path.exists() {
                Some(path_content_identity(&audit_path)?)
            } else {
                None
            },
            trusted,
            trusted_reason: if trusted {
                Some(format!(
                    "trusted importer source; audit risk {}",
                    audit.risk
                ))
            } else {
                None
            },
            imported_at_unix_ms: Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
            ),
        },
    );
    air_skill_runtime::write_skills_lock(&lock_path, &lock)
}

pub fn resolve_skill_run_metadata(reference: &str) -> Result<CodeRunSkill> {
    let skill = resolve_skill(reference)?;
    skill_metadata(&skill)
}

fn discover_skills() -> Result<Vec<ResolvedSkill>> {
    let mut skills = Vec::new();
    for root in skill_roots() {
        if !root.exists() {
            continue;
        }
        collect_skill_manifests_from_dir(&root, &mut skills)?;
        collect_skill_manifests_from_dir(&root.join("vendor"), &mut skills)?;
    }
    let mut seen = BTreeSet::new();
    skills.retain(|skill| seen.insert(skill.manifest.id.clone()));
    skills.sort_by(|left, right| left.manifest.id.cmp(&right.manifest.id));
    Ok(skills)
}

fn collect_skill_manifests_from_dir(root: &Path, skills: &mut Vec<ResolvedSkill>) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let manifest = entry.path().join(SKILL_MANIFEST);
        if manifest.exists() {
            skills.push(read_skill_manifest(&manifest)?);
        }
    }
    Ok(())
}

fn find_skill_manifest_by_id(skill_id: &str) -> Result<PathBuf> {
    for root in skill_roots() {
        for candidate in [
            root.join(skill_id).join(SKILL_MANIFEST),
            root.join("vendor").join(skill_id).join(SKILL_MANIFEST),
        ] {
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }
    bail!("skill manifest not found")
}

fn skill_roots() -> Vec<PathBuf> {
    vec![
        PathBuf::from("skills"),
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("skills"),
    ]
}

fn read_skill_manifest(path: &Path) -> Result<ResolvedSkill> {
    let manifest_content =
        fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let manifest: AirSkillManifest = serde_yaml::from_str(&manifest_content)
        .with_context(|| format!("parse {}", path.display()))?;
    let manifest_dir = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    Ok(ResolvedSkill {
        manifest_path: path.to_path_buf(),
        manifest_dir,
        manifest,
    })
}

fn manifest_path_for_existing_path(path: &Path) -> Result<PathBuf> {
    if path.is_dir() {
        let manifest = path.join(SKILL_MANIFEST);
        if manifest.exists() {
            return Ok(manifest);
        }
        bail!(
            "skill directory missing {SKILL_MANIFEST}: {}",
            path.display()
        );
    }
    if path.file_name().and_then(|name| name.to_str()) == Some(SKILL_MANIFEST) {
        return Ok(path.to_path_buf());
    }
    bail!(
        "skill path must be a directory or {SKILL_MANIFEST}: {}",
        path.display()
    )
}

fn validate_resolved_skill(skill: &ResolvedSkill) -> Result<Value> {
    if skill.manifest.schema.as_deref().unwrap_or(SKILL_SCHEMA) != SKILL_SCHEMA {
        bail!(
            "unsupported skill schema `{}`",
            skill.manifest.schema.as_deref().unwrap_or_default()
        );
    }
    if skill.manifest.id.trim().is_empty() {
        bail!("skill id must not be empty");
    }
    let effective = effective_execution(skill)?;
    if !effective.profile.exists() {
        bail!(
            "skill effective workflow profile does not exist: {}",
            effective.profile.display()
        );
    }
    if let Some(tool_config) = &effective.tool_config {
        if !tool_config.exists() {
            bail!(
                "skill effective tool config does not exist: {}",
                tool_config.display()
            );
        }
    }
    for file in &skill.manifest.instructions.files {
        let path = skill.manifest_dir.join(file);
        if !path.exists() {
            bail!("skill instruction file does not exist: {}", path.display());
        }
    }
    let allow = skill
        .manifest
        .capabilities
        .allow
        .iter()
        .collect::<BTreeSet<_>>();
    let conflicts = skill
        .manifest
        .capabilities
        .deny
        .iter()
        .filter(|capability| allow.contains(*capability))
        .cloned()
        .collect::<Vec<_>>();
    if !conflicts.is_empty() {
        bail!("skill capability appears in both allow and deny: {conflicts:?}");
    }
    if skill.manifest.mode == SkillMode::Instruction {
        if !skill.manifest.capabilities.allow.is_empty()
            || !skill.manifest.capabilities.deny.is_empty()
        {
            bail!(
                "instruction skill `{}` must not declare enforceable capability allow/deny; execution policy belongs to its host executor skill",
                skill.manifest.id
            );
        }
        if skill.manifest.tools.config.is_some() || skill.manifest.workflow.is_some() {
            bail!(
                "instruction skill `{}` must not declare workflow/tools execution settings; execution comes from host `{}`",
                skill.manifest.id,
                effective.host_skill.as_deref().unwrap_or("code-agent")
            );
        }
    } else {
        validate_executor_capabilities(skill, &effective.capabilities)?;
    }
    Ok(json!({
        "mode": skill.manifest.mode.as_str(),
        "host": effective.host_skill,
        "profile_exists": true,
        "tool_config_exists": effective.tool_config.is_some(),
        "instruction_files": skill.manifest.instructions.files.len(),
        "capability_allow": skill.manifest.capabilities.allow,
        "capability_deny": skill.manifest.capabilities.deny,
        "effective_capabilities": effective.capabilities,
    }))
}

fn instruction_host_id(manifest: &AirSkillManifest) -> &str {
    manifest
        .host
        .as_ref()
        .map(|host| host.skill.as_str())
        .unwrap_or(DEFAULT_EXECUTOR_SKILL)
}

fn validate_executor_capabilities(
    skill: &ResolvedSkill,
    effective_capabilities: &[String],
) -> Result<()> {
    let allow = skill
        .manifest
        .capabilities
        .allow
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let deny = skill
        .manifest
        .capabilities
        .deny
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let denied = effective_capabilities
        .iter()
        .filter(|capability| deny.contains(*capability))
        .cloned()
        .collect::<Vec<_>>();
    if !denied.is_empty() {
        bail!(
            "executor skill `{}` effective capabilities hit denied capabilities: {denied:?}",
            skill.manifest.id
        );
    }
    let missing = effective_capabilities
        .iter()
        .filter(|capability| !allow.contains(*capability))
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        bail!(
            "executor skill `{}` must allow every effective execution capability; missing {missing:?}",
            skill.manifest.id
        );
    }
    Ok(())
}

fn audit_resolved_skill(skill: &ResolvedSkill) -> Result<SkillAudit> {
    let audit = air_skill_runtime::audit_manifest_path(&skill.manifest_path)?;
    Ok(serde_json::from_value(serde_json::to_value(audit)?)?)
}

fn is_trusted_import_source(source: &str) -> bool {
    air_skill_runtime::is_trusted_import_source(source)
}

fn skill_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_skill_files(root, root, &mut files)?;
    Ok(files)
}

fn collect_skill_files(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap_or(&path);
        if relative.starts_with(".git")
            || relative.starts_with("target")
            || relative.starts_with("node_modules")
            || relative == Path::new("audit.json")
            || relative == Path::new("source.json")
            || relative == Path::new(air_skill_runtime::SKILLS_LOCK)
        {
            continue;
        }
        if entry.file_type()?.is_dir() {
            collect_skill_files(root, &path, files)?;
        } else if entry.file_type()?.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn is_executable_script_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("sh" | "bash" | "zsh" | "ps1" | "py" | "js" | "ts")
    )
}

fn import_source_to_temp(source: &str, temp_dir: &Path) -> Result<PathBuf> {
    let source_path = PathBuf::from(source);
    if source_path.exists() {
        if source_path.is_dir() {
            let destination = temp_dir.join("source");
            copy_dir_all(&source_path, &destination)?;
            return Ok(destination);
        }
        if source_path
            .extension()
            .and_then(|extension| extension.to_str())
            == Some("zip")
        {
            return unzip_to_temp(&source_path, temp_dir);
        }
        bail!("skill import source must be a directory, git URL, or zip: {source}");
    }
    if let Some(github_tree) = parse_github_tree_url(source) {
        let destination = temp_dir.join("repo");
        run_status(
            Command::new("git")
                .args(["clone", "--depth", "1", "--branch"])
                .arg(&github_tree.branch)
                .arg(&github_tree.repo_url)
                .arg(&destination),
            "git clone skill source",
        )?;
        let skill_dir = destination.join(&github_tree.subpath);
        if !skill_dir.exists() {
            bail!(
                "GitHub tree skill path does not exist after clone: {}",
                github_tree.subpath.display()
            );
        }
        return Ok(skill_dir);
    }
    if source.ends_with(".git") || source.contains("github.com") {
        let destination = temp_dir.join("repo");
        run_status(
            Command::new("git")
                .args(["clone", "--depth", "1", source])
                .arg(&destination),
            "git clone skill source",
        )?;
        return Ok(destination);
    }
    if source.starts_with("http://") || source.starts_with("https://") {
        let zip_path = temp_dir.join("source.zip");
        run_status(
            Command::new("curl")
                .args(["-L", "--fail", source, "-o"])
                .arg(&zip_path),
            "download skill zip",
        )?;
        return unzip_to_temp(&zip_path, temp_dir);
    }
    bail!("skill import source does not exist: {source}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitHubTreeSource {
    repo_url: String,
    branch: String,
    subpath: PathBuf,
}

fn parse_github_tree_url(source: &str) -> Option<GitHubTreeSource> {
    let clean = source.split(['?', '#']).next()?.trim_end_matches('/');
    let path = clean
        .strip_prefix("https://github.com/")
        .or_else(|| clean.strip_prefix("http://github.com/"))?;
    let segments = path.split('/').collect::<Vec<_>>();
    if segments.len() < 5 || segments[2] != "tree" {
        return None;
    }
    let owner = segments[0];
    let repo = segments[1].trim_end_matches(".git");
    let branch = segments[3].to_string();
    let mut subpath = PathBuf::new();
    for segment in &segments[4..] {
        subpath.push(segment);
    }
    Some(GitHubTreeSource {
        repo_url: format!("https://github.com/{owner}/{repo}.git"),
        branch,
        subpath,
    })
}

fn unzip_to_temp(zip_path: &Path, temp_dir: &Path) -> Result<PathBuf> {
    let destination = temp_dir.join("unzipped");
    fs::create_dir_all(&destination)
        .with_context(|| format!("create {}", destination.display()))?;
    run_status(
        Command::new("unzip")
            .args(["-q"])
            .arg(zip_path)
            .args(["-d"])
            .arg(&destination),
        "unzip skill source",
    )?;
    single_child_dir_or_self(&destination)
}

fn single_child_dir_or_self(path: &Path) -> Result<PathBuf> {
    let children = fs::read_dir(path)?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    if children.len() == 1 && children[0].file_type()?.is_dir() {
        Ok(children[0].path())
    } else {
        Ok(path.to_path_buf())
    }
}

fn ensure_import_manifest(source: &str, out: &Path) -> Result<()> {
    let manifest_path = out.join(SKILL_MANIFEST);
    if manifest_path.exists() {
        return Ok(());
    }
    let frontmatter = skill_frontmatter_for_dir(out).unwrap_or_default();
    let id = out
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("imported-skill")
        .to_string();
    let description = frontmatter
        .description
        .clone()
        .or_else(|| frontmatter.title.clone())
        .or_else(|| frontmatter.name.clone())
        .unwrap_or_else(|| format!("Imported AIR skill wrapper for {source}"));
    let routing = SkillRouting {
        summary: Some(description.clone()),
        triggers: derived_skill_triggers(&id, &frontmatter, &description),
        repo_markers: frontmatter.repo_markers.iter().map(PathBuf::from).collect(),
        languages: frontmatter.languages.clone(),
        task_types: frontmatter.task_types.clone(),
        ..Default::default()
    };
    let manifest = AirSkillManifest {
        schema: Some(SKILL_SCHEMA.to_string()),
        id,
        mode: SkillMode::Instruction,
        version: Some("0.1.0".to_string()),
        description: Some(description),
        source: Some(json!({
            "type": "imported",
            "original": source,
            "frontmatter": frontmatter,
        })),
        instructions: SkillInstructions {
            files: find_instruction_files(out)?,
        },
        capabilities: SkillCapabilities::default(),
        tools: SkillTools { config: None },
        host: Some(SkillHost {
            skill: "code-agent".to_string(),
        }),
        workflow: None,
        routing,
        verification: json!({
            "mode": "instruction_only",
            "scripts": "disabled",
        }),
    };
    fs::write(&manifest_path, serde_yaml::to_string(&manifest)?)
        .with_context(|| format!("write {}", manifest_path.display()))
}

fn derived_skill_triggers(
    id: &str,
    frontmatter: &SkillFrontmatter,
    description: &str,
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut triggers = Vec::new();
    for trigger in &frontmatter.triggers {
        let trigger = trigger.trim().to_ascii_lowercase();
        if !trigger.is_empty() && seen.insert(trigger.clone()) {
            triggers.push(trigger);
        }
    }
    for value in frontmatter.name.iter().chain(frontmatter.title.iter()) {
        for token in value
            .split(|character: char| !character.is_ascii_alphanumeric())
            .map(str::trim)
            .filter(|token| token.len() >= 3)
        {
            let token = token.to_ascii_lowercase();
            if seen.insert(token.clone()) {
                triggers.push(token);
            }
        }
    }
    for token in route_tokens(id)
        .into_iter()
        .chain(route_tokens(description))
        .filter(|token| !route_stop_words().contains(token.as_str()))
        .take(12)
    {
        if seen.insert(token.clone()) {
            triggers.push(token);
        }
    }
    triggers
}

fn route_stop_words() -> BTreeSet<&'static str> {
    BTreeSet::from([
        "this", "that", "with", "when", "from", "using", "code", "skill", "workflow", "writing",
        "existing",
    ])
}

fn find_instruction_files(root: &Path) -> Result<Vec<PathBuf>> {
    for name in ["SKILL.md", "README.md"] {
        if root.join(name).exists() {
            return Ok(vec![PathBuf::from(name)]);
        }
    }
    Ok(Vec::new())
}

fn copy_dir_all(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination).with_context(|| format!("create {}", destination.display()))?;
    for entry in fs::read_dir(source).with_context(|| format!("read {}", source.display()))? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&source_path, &destination_path)?;
        } else {
            fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "copy {} to {}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn run_status(command: &mut Command, label: &str) -> Result<()> {
    let status = command.status().with_context(|| format!("run {label}"))?;
    if !status.success() {
        bail!("{label} failed with status {status}");
    }
    Ok(())
}

fn directory_identity(root: &Path) -> Result<String> {
    let mut items = Vec::new();
    for file in skill_files(root)? {
        let relative = path_ref(file.strip_prefix(root).unwrap_or(&file));
        let identity = path_content_identity(&file)?;
        items.push(json!({ "path": relative, "identity": identity }));
    }
    items.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    let encoded = serde_json::to_vec(&items)?;
    Ok(format!(
        "sha256:{}",
        air_code_artifact::sha256_hex(&encoded)
    ))
}

fn skill_file_identity_map(root: &Path) -> Result<BTreeMap<String, String>> {
    let mut files = BTreeMap::new();
    for file in skill_files(root)? {
        let relative = path_ref(file.strip_prefix(root).unwrap_or(&file));
        files.insert(relative, path_content_identity(&file)?);
    }
    Ok(files)
}

fn skill_file_diff(
    current: &BTreeMap<String, String>,
    candidate: &BTreeMap<String, String>,
) -> Value {
    let current_paths = current.keys().cloned().collect::<BTreeSet<_>>();
    let candidate_paths = candidate.keys().cloned().collect::<BTreeSet<_>>();
    let added = candidate_paths
        .difference(&current_paths)
        .cloned()
        .collect::<Vec<_>>();
    let removed = current_paths
        .difference(&candidate_paths)
        .cloned()
        .collect::<Vec<_>>();
    let changed = current_paths
        .intersection(&candidate_paths)
        .filter(|path| current.get(*path) != candidate.get(*path))
        .cloned()
        .collect::<Vec<_>>();
    json!({
        "added": added,
        "removed": removed,
        "changed": changed,
    })
}

fn skill_file_diff_changed(diff: &Value) -> bool {
    ["added", "removed", "changed"].iter().any(|key| {
        diff.get(*key)
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
    })
}

fn skill_capability_set(manifest: &AirSkillManifest) -> BTreeSet<String> {
    manifest
        .capabilities
        .allow
        .iter()
        .map(|capability| format!("allow:{capability}"))
        .chain(
            manifest
                .capabilities
                .deny
                .iter()
                .map(|capability| format!("deny:{capability}")),
        )
        .collect()
}

#[derive(Debug, Clone)]
struct EffectiveSkillExecution {
    host_skill: Option<String>,
    profile: PathBuf,
    tool_config: Option<PathBuf>,
    capabilities: Vec<String>,
}

fn effective_execution(skill: &ResolvedSkill) -> Result<EffectiveSkillExecution> {
    if skill.manifest.mode == SkillMode::Instruction {
        let host_skill = skill.host_skill_id().unwrap_or("code-agent").to_string();
        let executor = resolve_skill(&host_skill)?;
        if executor.manifest.mode != SkillMode::Executor {
            bail!("instruction skill host `{host_skill}` must be an executor skill");
        }
        let host_profile = executor.executor_profile_path()?;
        let host_tool_config = effective_tool_config_path(&executor, &host_profile)?;
        let mut host_capabilities = profile_capabilities(&host_profile)?;
        if let Some(tool_config) = &host_tool_config {
            host_capabilities.extend(tool_config_capabilities(tool_config)?);
        }
        host_capabilities.sort();
        host_capabilities.dedup();
        return Ok(EffectiveSkillExecution {
            host_skill: Some(host_skill),
            profile: host_profile,
            tool_config: host_tool_config,
            capabilities: host_capabilities,
        });
    }
    let profile = skill.executor_profile_path()?;
    let tool_config = effective_tool_config_path(skill, &profile)?;
    let mut capabilities = profile_capabilities(&profile)?;
    if let Some(tool_config) = &tool_config {
        capabilities.extend(tool_config_capabilities(tool_config)?);
    }
    capabilities.sort();
    capabilities.dedup();
    Ok(EffectiveSkillExecution {
        host_skill: None,
        profile,
        tool_config,
        capabilities,
    })
}

fn profile_capabilities(profile: &Path) -> Result<Vec<String>> {
    let mut capabilities = explain_metadata_for_profile(profile)?.capabilities;
    capabilities.sort();
    capabilities.dedup();
    Ok(capabilities)
}

fn effective_tool_config_path(skill: &ResolvedSkill, profile: &Path) -> Result<Option<PathBuf>> {
    if let Some(tool_config) = skill.tool_config_path() {
        return Ok(Some(tool_config));
    }
    let profile_config = read_native_loop_profile(&profile.to_path_buf())?;
    Ok(profile_config
        .tool_config
        .map(|path| resolve_profile_path(profile, &path)))
}

fn tool_config_capabilities(path: &Path) -> Result<Vec<String>> {
    let content = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let value: Value =
        serde_json::from_str(&content).with_context(|| format!("parse {}", path.display()))?;
    let mut capabilities = Vec::new();
    let Some(tools) = value.get("tools").and_then(Value::as_object) else {
        return Ok(capabilities);
    };
    for tool in tools.values() {
        if let Some(capability) = tool.get("capability").and_then(Value::as_str) {
            capabilities.push(capability.to_string());
        }
    }
    capabilities.sort();
    capabilities.dedup();
    Ok(capabilities)
}

fn preload_skill_for_run(skill: &ResolvedSkill, audit: &SkillAudit) -> Result<Vec<Value>> {
    if skill.manifest.id == "code-agent" {
        return Ok(Vec::new());
    }
    Ok(vec![json!({
        "id": skill.manifest.id,
        "mode": skill.manifest.mode.as_str(),
        "audit_risk": audit.risk,
        "manifest": path_ref(&skill.manifest_path),
        "root": path_ref(&skill.manifest_dir),
        "scripts": skill_script_assets(skill)?,
        "instructions": load_instruction_files(skill, 65_536)?,
    })])
}

fn skill_script_assets(skill: &ResolvedSkill) -> Result<Vec<Value>> {
    let mut scripts = Vec::new();
    for path in skill_files(&skill.manifest_dir)? {
        if !is_executable_script_path(&path) {
            continue;
        }
        let relative = path.strip_prefix(&skill.manifest_dir).unwrap_or(&path);
        scripts.push(json!({
            "path": path_ref(relative),
            "absolute_path": path_ref(&path),
        }));
    }
    scripts.sort_by(|left, right| {
        left["path"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["path"].as_str().unwrap_or_default())
    });
    Ok(scripts)
}

fn load_instruction_files(skill: &ResolvedSkill, max_bytes: usize) -> Result<Vec<Value>> {
    let mut remaining = max_bytes;
    let mut loaded = Vec::new();
    for relative in &skill.manifest.instructions.files {
        if remaining == 0 {
            break;
        }
        let path = skill.manifest_dir.join(relative);
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let returned = remaining.min(bytes.len());
        let content = String::from_utf8_lossy(&bytes[..returned]).to_string();
        remaining = remaining.saturating_sub(returned);
        loaded.push(json!({
            "path": path_ref(&path),
            "sha256": path_content_identity(&path)?,
            "bytes": bytes.len(),
            "bytes_returned": returned,
            "truncated": returned < bytes.len(),
            "content": content,
        }));
    }
    Ok(loaded)
}

fn render_preloaded_skill_instructions(preloaded_skills: &[Value]) -> String {
    let mut output = String::from("<skill_instructions>");
    for skill in preloaded_skills {
        let id = skill.get("id").and_then(Value::as_str).unwrap_or("unknown");
        let mode = skill
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("instruction");
        output.push_str(&format!("\n<skill id=\"{id}\" mode=\"{mode}\">"));
        if let Some(root) = skill.get("root").and_then(Value::as_str) {
            output.push_str(&format!("\n<skill_root>{root}</skill_root>"));
            output.push_str(
                "\n<asset_note>Paths mentioned by this skill are relative to skill_root. Use absolute_path when invoking bundled scripts from another workspace.</asset_note>",
            );
        }
        if let Some(scripts) = skill.get("scripts").and_then(Value::as_array) {
            if !scripts.is_empty() {
                output.push_str("\n<scripts>");
                for script in scripts {
                    let path = script
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let absolute_path = script
                        .get("absolute_path")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    output.push_str(&format!(
                        "\n<script path=\"{path}\" absolute_path=\"{absolute_path}\" />"
                    ));
                }
                output.push_str("\n</scripts>");
            }
        }
        if let Some(instructions) = skill.get("instructions").and_then(Value::as_array) {
            for instruction in instructions {
                let path = instruction
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or("instruction");
                let content = instruction
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                output.push_str(&format!(
                    "\n<instruction path=\"{path}\">\n{content}\n</instruction>"
                ));
            }
        }
        output.push_str("\n</skill>");
    }
    output.push_str("\n</skill_instructions>");
    output
}

fn preloaded_instruction_files(preloaded_skills: &[Value]) -> Vec<Value> {
    let mut files = Vec::new();
    for skill in preloaded_skills {
        let skill_id = skill.get("id").and_then(Value::as_str).unwrap_or("unknown");
        let Some(instructions) = skill.get("instructions").and_then(Value::as_array) else {
            continue;
        };
        for instruction in instructions {
            files.push(json!({
                "skill": skill_id,
                "path": instruction.get("path").cloned().unwrap_or(Value::Null),
                "sha256": instruction.get("sha256").cloned().unwrap_or(Value::Null),
                "bytes": instruction.get("bytes").cloned().unwrap_or(Value::Null),
                "truncated": instruction.get("truncated").cloned().unwrap_or(Value::Bool(false)),
            }));
        }
    }
    files
}

fn skill_metadata(skill: &ResolvedSkill) -> Result<CodeRunSkill> {
    let effective = effective_execution(skill)?;
    skill_metadata_with_context(skill, None, &effective)
}

fn skill_metadata_with_context(
    skill: &ResolvedSkill,
    audit: Option<&SkillAudit>,
    effective: &EffectiveSkillExecution,
) -> Result<CodeRunSkill> {
    let audit_risk = read_audit_risk(&skill.manifest_dir.join("audit.json"));
    Ok(CodeRunSkill {
        id: skill.manifest.id.clone(),
        version: skill.manifest.version.clone(),
        mode: Some(skill.manifest.mode.as_str().to_string()),
        source: skill.source_label(),
        manifest: Some(path_ref(&skill.manifest_path)),
        manifest_sha256: Some(path_content_identity(&skill.manifest_path)?),
        audit_risk: audit.map(|audit| audit.risk.clone()).or(audit_risk),
        effective_capabilities: effective.capabilities.clone(),
    })
}

fn read_audit_risk(path: &Path) -> Option<String> {
    let content = fs::read(path).ok()?;
    let audit: SkillAudit = serde_json::from_slice(&content).ok()?;
    Some(audit.risk)
}

impl ResolvedSkill {
    fn tool_config_path(&self) -> Option<PathBuf> {
        self.manifest
            .tools
            .config
            .as_ref()
            .map(|path| resolve_skill_path(&self.manifest_dir, path))
    }

    fn source_label(&self) -> Option<String> {
        self.manifest.source.as_ref().and_then(|source| {
            source
                .get("type")
                .and_then(Value::as_str)
                .or_else(|| source.as_str())
                .map(str::to_string)
        })
    }

    fn list_item(&self) -> Result<SkillListItem> {
        let effective = effective_execution(self)?;
        Ok(SkillListItem {
            id: self.manifest.id.clone(),
            mode: self.manifest.mode.as_str().to_string(),
            version: self.manifest.version.clone(),
            description: self.manifest.description.clone().or_else(|| {
                skill_frontmatter_for_dir(&self.manifest_dir).and_then(|front| front.description)
            }),
            manifest: path_ref(&self.manifest_path),
            profile: path_ref(&effective.profile),
            tool_config: effective.tool_config.map(|path| path_ref(&path)),
            host: self.host_skill_id().map(str::to_string),
            explain: format!("air skill explain {}", self.manifest.id),
        })
    }

    fn host_skill_id(&self) -> Option<&str> {
        self.manifest.host.as_ref().map(|host| host.skill.as_str())
    }

    fn executor_profile_path(&self) -> Result<PathBuf> {
        let Some(workflow) = &self.manifest.workflow else {
            bail!(
                "executor skill `{}` must declare workflow.profile",
                self.manifest.id
            );
        };
        Ok(resolve_skill_path(&self.manifest_dir, &workflow.profile))
    }
}

fn resolve_skill_path(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn path_ref(path: &Path) -> String {
    path_ref_to_input_string(path)
}

fn print_json(value: &Value) -> Result<()> {
    serde_json::to_writer_pretty(std::io::stdout(), value)?;
    println!();
    Ok(())
}

fn path_ref_to_input_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn build_input(
    task: String,
    verification_command: Option<String>,
    requires_edit: bool,
) -> Map<String, Value> {
    let mut input = Map::new();
    input.insert("task".to_string(), Value::String(task));
    input.insert(
        "verification_command".to_string(),
        Value::String(verification_command.unwrap_or_default()),
    );
    input.insert("requires_edit".to_string(), Value::Bool(requires_edit));
    input
}

pub fn merge_skill_ids(primary: &[String], additional: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    primary
        .iter()
        .map(String::as_str)
        .chain(additional.iter().flat_map(|value| value.split(',')))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter_map(|value| {
            if seen.insert(value.to_string()) {
                Some(value.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn code_profile_explain(
    skill_id: &str,
    profile: &Path,
    input: &Map<String, Value>,
) -> Result<Value> {
    let metadata = explain_metadata_for_profile(profile)?;
    Ok(json!({
        "command": "run",
        "skill": skill_id,
        "will_run": false,
        "profile": path_ref_to_input_string(profile),
        "plan": metadata.plan.as_ref().map(|path| path_ref_to_input_string(path)),
        "store": metadata.store.as_ref().map(|path| path_ref_to_input_string(path)),
        "native_loop": metadata.native_loop,
        "capabilities": metadata.capabilities,
        "read_only": metadata.read_only,
        "writes_workspace": metadata.writes_workspace,
        "input": Value::Object(input.clone()),
    }))
}

fn explain_metadata_for_profile(profile: &Path) -> Result<CodeExplainMetadata> {
    let profile_path = profile.to_path_buf();
    let profile = read_native_loop_profile(&profile_path)?;
    if profile.plan.is_some() || profile.store.is_some() {
        anyhow::bail!(
            "legacy plan/store profile {} is no longer supported; use native_loop and capabilities",
            profile_path.display()
        );
    }
    let plan_path = profile
        .plan
        .as_ref()
        .map(|plan| resolve_profile_path(&profile_path, plan));
    let store_path = profile
        .store
        .as_ref()
        .map(|store| resolve_profile_path(&profile_path, store));
    let mut capabilities = profile.capabilities;
    if capabilities.is_empty() {
        anyhow::bail!(
            "native profile {} must declare capabilities when plan is omitted",
            profile_path.display()
        );
    }
    capabilities.sort();
    capabilities.dedup();
    let writes_workspace = capabilities
        .iter()
        .any(|capability| capability == "file.write");
    Ok(CodeExplainMetadata {
        plan: plan_path,
        store: store_path,
        native_loop: profile.native_loop,
        capabilities,
        read_only: !writes_workspace,
        writes_workspace,
    })
}

fn read_native_loop_profile(path: &PathBuf) -> Result<NativeLoopProfile> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read native loop profile {}", path.display()))?;
    serde_yaml::from_str(&source).with_context(|| {
        format!(
            "failed to parse native loop profile YAML {}",
            path.display()
        )
    })
}

fn resolve_profile_path(profile_path: &Path, path: &PathBuf) -> PathBuf {
    if path.is_absolute() {
        return path.clone();
    }
    profile_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(path)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_builtin_code_agent_skill_manifest() {
        let skill = resolve_skill("code-agent").unwrap();
        assert_eq!(skill.manifest.id, "code-agent");
        assert!(skill
            .executor_profile_path()
            .unwrap()
            .ends_with("edit.air-profile.yaml"));
        validate_resolved_skill(&skill).unwrap();
    }

    #[test]
    fn audits_remote_shell_patterns() {
        let temp = std::env::temp_dir().join(format!(
            "air-skill-audit-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp).unwrap();
        fs::write(temp.join("SKILL.md"), "Run curl | sh to install").unwrap();
        fs::write(
            temp.join(SKILL_MANIFEST),
            r#"
schema: air.skill.v1
id: unsafe
workflow:
  profile: profile.air-profile.yaml
instructions:
  files: [SKILL.md]
"#,
        )
        .unwrap();
        fs::write(
            temp.join("profile.air-profile.yaml"),
            "plan: plan\nstore: store\n",
        )
        .unwrap();
        let skill = read_skill_manifest(&temp.join(SKILL_MANIFEST)).unwrap();
        let audit = audit_resolved_skill(&skill).unwrap();
        assert_eq!(audit.risk, "critical");
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn trusted_anthropic_skill_source_can_run_with_recorded_high_findings() {
        let temp = std::env::temp_dir().join(format!(
            "air-skill-trusted-source-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp).unwrap();
        fs::write(
            temp.join("SKILL.md"),
            "Use process.env only as documentation.",
        )
        .unwrap();
        fs::write(
            temp.join(SKILL_MANIFEST),
            r#"
schema: air.skill.v1
id: trusted-doc-skill
mode: instruction
host:
  skill: code-agent
source:
  type: imported
  original: https://github.com/anthropics/skills#skills/docx
instructions:
  files: [SKILL.md]
"#,
        )
        .unwrap();
        fs::write(
            temp.join("source.json"),
            serde_json::to_vec_pretty(&json!({
                "source": "https://github.com/anthropics/skills",
                "sha256": directory_identity(&temp).unwrap(),
            }))
            .unwrap(),
        )
        .unwrap();
        let mut lock = air_skill_runtime::SkillsLock::default();
        lock.skills.insert(
            "trusted-doc-skill".to_string(),
            air_skill_runtime::SkillLockEntry {
                manifest: path_ref(&temp.join(SKILL_MANIFEST)),
                source: Some("https://github.com/anthropics/skills".to_string()),
                sha256: directory_identity(&temp).unwrap(),
                audit_sha256: None,
                trusted: true,
                trusted_reason: Some("test trust decision".to_string()),
                imported_at_unix_ms: None,
            },
        );
        air_skill_runtime::write_skills_lock(&temp.join(air_skill_runtime::SKILLS_LOCK), &lock)
            .unwrap();
        let skill = read_skill_manifest(&temp.join(SKILL_MANIFEST)).unwrap();
        let audit = audit_resolved_skill(&skill).unwrap();
        assert_eq!(audit.risk, "high");
        assert!(audit.trusted_source);
        assert!(audit.allowed_to_run);
        assert!(audit.allowed_reason.is_some());
        assert!(audit
            .findings
            .iter()
            .any(|finding| finding.severity == "high"));
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn manifest_cannot_self_declare_trusted_source() {
        let temp = std::env::temp_dir().join(format!(
            "air-skill-trust-spoof-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp).unwrap();
        fs::write(
            temp.join("SKILL.md"),
            "Use process.env only as documentation.",
        )
        .unwrap();
        fs::write(
            temp.join(SKILL_MANIFEST),
            r#"
schema: air.skill.v1
id: spoofed-trust
mode: instruction
host:
  skill: code-agent
source:
  type: imported
  original: https://github.com/anthropics/skills-fake
  trusted: true
instructions:
  files: [SKILL.md]
"#,
        )
        .unwrap();

        let skill = read_skill_manifest(&temp.join(SKILL_MANIFEST)).unwrap();
        let audit = audit_resolved_skill(&skill).unwrap();
        assert_eq!(audit.risk, "high");
        assert!(!audit.trusted_source);
        assert!(!audit.allowed_to_run);
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn trusted_import_source_matches_exact_github_repo() {
        assert!(is_trusted_import_source(
            "https://github.com/anthropics/skills"
        ));
        assert!(is_trusted_import_source(
            "https://github.com/anthropics/skills/tree/main/skills/docx"
        ));
        assert!(!is_trusted_import_source(
            "https://github.com/anthropics/skills-fake"
        ));
        assert!(!is_trusted_import_source(
            "https://example.com/anthropics/skills"
        ));
    }

    #[test]
    fn audits_external_urls_installers_and_mutating_scripts() {
        let temp = std::env::temp_dir().join(format!(
            "air-skill-audit-pattern-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp).unwrap();
        fs::write(
            temp.join("SKILL.md"),
            "Fetch https://example.com/docs, run npm install, then chmod +x helper.sh.",
        )
        .unwrap();
        fs::write(
            temp.join(SKILL_MANIFEST),
            r#"
schema: air.skill.v1
id: risky-docs
mode: instruction
host:
  skill: code-agent
instructions:
  files: [SKILL.md]
"#,
        )
        .unwrap();
        let skill = read_skill_manifest(&temp.join(SKILL_MANIFEST)).unwrap();
        let audit = audit_resolved_skill(&skill).unwrap();
        assert_eq!(audit.risk, "medium");
        assert!(audit
            .findings
            .iter()
            .any(|finding| finding.reason.contains("external URL")));
        assert!(audit
            .findings
            .iter()
            .any(|finding| finding.reason.contains("installer command")));
        assert!(audit
            .findings
            .iter()
            .any(|finding| finding.reason.contains("filesystem mutation")));
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn parses_skill_frontmatter_metadata() {
        let frontmatter = parse_skill_frontmatter(
            r#"---
name: tdd-workflow
description: Use test-driven development for bug fixes.
triggers:
  - tdd
  - failing test
repo_markers:
  - package.json
languages: rust, typescript
allowed-tools: Read, Bash
---

# TDD
"#,
        )
        .unwrap();
        assert_eq!(frontmatter.name.as_deref(), Some("tdd-workflow"));
        assert_eq!(
            frontmatter.description.as_deref(),
            Some("Use test-driven development for bug fixes.")
        );
        assert_eq!(frontmatter.triggers, vec!["tdd", "failing test"]);
        assert_eq!(frontmatter.repo_markers, vec!["package.json"]);
        assert_eq!(frontmatter.languages, vec!["rust", "typescript"]);
        assert_eq!(frontmatter.allowed_tools, vec!["Read", "Bash"]);
    }

    #[test]
    fn imported_skill_manifest_uses_frontmatter_for_routing() {
        let temp = std::env::temp_dir().join(format!(
            "air-skill-frontmatter-import-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp).unwrap();
        fs::write(
            temp.join("SKILL.md"),
            r#"---
name: custom-tdd
description: Use TDD for parser bugs.
triggers:
  - parser bug
---

# Custom TDD
"#,
        )
        .unwrap();
        ensure_import_manifest("local", &temp).unwrap();
        let skill = read_skill_manifest(&temp.join(SKILL_MANIFEST)).unwrap();
        assert_eq!(skill.manifest.mode, SkillMode::Instruction);
        assert_eq!(
            skill.manifest.description.as_deref(),
            Some("Use TDD for parser bugs.")
        );
        assert!(skill
            .manifest
            .routing
            .triggers
            .iter()
            .any(|trigger| trigger == "parser bug"));
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn discovers_collection_skills_from_anthropic_style_repo() {
        let temp = std::env::temp_dir().join(format!(
            "air-skill-collection-import-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let skills_dir = temp.join("skills");
        fs::create_dir_all(skills_dir.join("First Skill")).unwrap();
        fs::create_dir_all(skills_dir.join("second-skill")).unwrap();
        fs::create_dir_all(temp.join("template")).unwrap();
        fs::write(
            skills_dir.join("First Skill").join("SKILL.md"),
            r#"---
name: first-skill
description: First imported skill.
---

# First
"#,
        )
        .unwrap();
        fs::write(
            skills_dir.join("second-skill").join("SKILL.md"),
            r#"---
description: Second imported skill.
---

# Second
"#,
        )
        .unwrap();
        fs::write(temp.join("template").join("SKILL.md"), "# Template").unwrap();

        let units = discover_import_units(&temp).unwrap();
        assert_eq!(
            units
                .iter()
                .map(|unit| unit.id.as_str())
                .collect::<Vec<_>>(),
            vec!["first-skill", "second-skill"]
        );
        assert!(units.iter().all(|unit| unit.relative.starts_with("skills")));
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn skill_id_merge_deduplicates_auto_and_forced_skills() {
        let merged = merge_skill_ids(
            &["tdd-workflow".to_string()],
            &["tdd-workflow,extra-skill".to_string()],
        );
        assert_eq!(merged, vec!["tdd-workflow", "extra-skill"]);
    }

    #[test]
    fn instruction_skill_preload_includes_instruction_content() {
        let temp = std::env::temp_dir().join(format!(
            "air-skill-preload-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp).unwrap();
        fs::write(temp.join("SKILL.md"), "Use tests before implementation.").unwrap();
        fs::write(
            temp.join(SKILL_MANIFEST),
            r#"
schema: air.skill.v1
id: tdd
mode: instruction
host:
  skill: code-agent
instructions:
  files: [SKILL.md]
"#,
        )
        .unwrap();
        let skill = read_skill_manifest(&temp.join(SKILL_MANIFEST)).unwrap();
        let audit = audit_resolved_skill(&skill).unwrap();
        let preloaded = preload_skill_for_run(&skill, &audit).unwrap();
        assert_eq!(preloaded.len(), 1);
        assert!(preloaded[0]
            .to_string()
            .contains("tests before implementation"));
        let task_prefix = render_preloaded_skill_instructions(&preloaded);
        let task = format!("{task_prefix}\n\nTask: fix bug");
        assert!(task.contains("<skill_instructions>"));
        assert!(task.contains("tests before implementation"));
        assert!(task.ends_with("Task: fix bug"));
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn preloaded_skill_instructions_include_script_asset_paths() {
        let temp = std::env::temp_dir().join(format!(
            "air-skill-script-assets-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(temp.join("scripts")).unwrap();
        fs::write(temp.join("SKILL.md"), "Run scripts/helper.py --help first.").unwrap();
        fs::write(temp.join("scripts/helper.py"), "print('help')").unwrap();
        fs::write(
            temp.join(SKILL_MANIFEST),
            r#"
schema: air.skill.v1
id: script-helper
mode: instruction
host:
  skill: code-agent
instructions:
  files: [SKILL.md]
"#,
        )
        .unwrap();
        let skill = read_skill_manifest(&temp.join(SKILL_MANIFEST)).unwrap();
        let audit = audit_resolved_skill(&skill).unwrap();
        let preloaded = preload_skill_for_run(&skill, &audit).unwrap();
        let rendered = render_preloaded_skill_instructions(&preloaded);
        assert!(rendered.contains("<skill_root>"));
        assert!(rendered.contains("<scripts>"));
        assert!(rendered.contains("scripts/helper.py"));
        assert!(rendered.contains("absolute_path="));
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn instruction_skill_cannot_claim_execution_capabilities() {
        let temp = std::env::temp_dir().join(format!(
            "air-skill-mode-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp).unwrap();
        fs::write(temp.join("SKILL.md"), "Read only").unwrap();
        fs::write(
            temp.join(SKILL_MANIFEST),
            r#"
schema: air.skill.v1
id: misleading
mode: instruction
host:
  skill: code-agent
instructions:
  files: [SKILL.md]
capabilities:
  deny: [file.write]
"#,
        )
        .unwrap();
        let skill = read_skill_manifest(&temp.join(SKILL_MANIFEST)).unwrap();
        let error = validate_resolved_skill(&skill).unwrap_err().to_string();
        assert!(error.contains("must not declare enforceable capability"));
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn instruction_skill_uses_host_execution_profile() {
        let skill = resolve_skill("tdd-workflow").unwrap();
        assert_eq!(skill.manifest.mode, SkillMode::Instruction);
        validate_resolved_skill(&skill).unwrap();

        let effective = effective_execution(&skill).unwrap();
        assert_eq!(effective.host_skill.as_deref(), Some("code-agent"));
        assert!(effective.profile.ends_with("edit.air-profile.yaml"));
        assert!(effective
            .tool_config
            .as_ref()
            .is_some_and(|path| path.ends_with("tools.json")));
    }

    #[test]
    fn instruction_skill_cannot_declare_workflow_or_tools() {
        let temp = std::env::temp_dir().join(format!(
            "air-skill-instruction-exec-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp).unwrap();
        fs::write(temp.join("SKILL.md"), "Use this as guidance only.").unwrap();
        fs::write(
            temp.join(SKILL_MANIFEST),
            r#"
schema: air.skill.v1
id: misleading-workflow
mode: instruction
host:
  skill: code-agent
instructions:
  files: [SKILL.md]
tools:
  config: tools.json
workflow:
  profile: profile.air-profile.yaml
"#,
        )
        .unwrap();
        let skill = read_skill_manifest(&temp.join(SKILL_MANIFEST)).unwrap();
        let error = validate_resolved_skill(&skill).unwrap_err().to_string();
        assert!(error.contains("must not declare workflow/tools"));
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn routes_tdd_tasks_to_tdd_workflow() {
        let route = route_skills_for_task("use TDD to refactor a small helper", 3).unwrap();
        assert_eq!(route.executor.id, "code-agent");
        assert!(route
            .instructions
            .iter()
            .any(|card| card.id == "tdd-workflow"));
        assert!(route.selected.iter().any(|card| card.id == "tdd-workflow"));
    }

    #[test]
    fn routes_review_tasks_to_review_agent() {
        let route = route_skills_for_task("review this MR for correctness", 3).unwrap();
        assert_eq!(route.executor.id, "review-agent");
        assert!(route
            .instructions
            .iter()
            .any(|card| card.id == "code-review" && card.host == "review-agent"));
    }

    #[test]
    fn generic_refactor_does_not_default_to_tdd_workflow() {
        let route = route_skills_for_task(
            "refactor the duplicated retry helper and run the existing tests",
            3,
        )
        .unwrap();
        assert!(!route
            .instructions
            .iter()
            .any(|card| card.id == "tdd-workflow"));
    }

    #[test]
    fn skill_composition_preloads_instruction_skills_on_code_agent() {
        let prepared = prepare_skill_composition(&["tdd-workflow".to_string()]).unwrap();
        assert_eq!(prepared.metadata.id, "code-agent");
        assert!(prepared.profile.ends_with("edit.air-profile.yaml"));
        let prefix = prepared.task_prefix.unwrap_or_default();
        assert!(prefix.contains("<skill_instructions>"));
        assert!(prefix.contains("tdd-workflow"));
        assert!(prepared.artifact_extra.contains_key("skill_composition"));
    }

    #[test]
    fn parses_github_tree_skill_url() {
        let parsed = parse_github_tree_url(
            "https://github.com/affaan-m/everything-claude-code/tree/main/skills/tdd-workflow",
        )
        .unwrap();
        assert_eq!(
            parsed.repo_url,
            "https://github.com/affaan-m/everything-claude-code.git"
        );
        assert_eq!(parsed.branch, "main");
        assert_eq!(parsed.subpath, PathBuf::from("skills/tdd-workflow"));
    }
}
