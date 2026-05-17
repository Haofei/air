use crate::code_agent::{
    build_input, code_profile_explain, explain_metadata_for_profile, path_ref_to_input_string,
    run_code_agent, CodeOptions,
};
use crate::code_artifact::{path_content_identity, CodeRunSkill};
use crate::profile::{read_run_plan_profile, resolve_profile_path};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const SKILL_MANIFEST: &str = "air-skill.yaml";
const SKILL_SCHEMA: &str = "air.skill.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AirSkillManifest {
    #[serde(default)]
    pub(crate) schema: Option<String>,
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) mode: SkillMode,
    #[serde(default)]
    pub(crate) version: Option<String>,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) source: Option<Value>,
    #[serde(default)]
    pub(crate) instructions: SkillInstructions,
    #[serde(default)]
    pub(crate) capabilities: SkillCapabilities,
    #[serde(default)]
    pub(crate) tools: SkillTools,
    #[serde(default)]
    pub(crate) host: Option<SkillHost>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) workflow: Option<SkillWorkflow>,
    #[serde(default)]
    pub(crate) routing: SkillRouting,
    #[serde(default)]
    pub(crate) verification: Value,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SkillMode {
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
pub(crate) struct SkillInstructions {
    #[serde(default)]
    pub(crate) files: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SkillCapabilities {
    #[serde(default)]
    pub(crate) allow: Vec<String>,
    #[serde(default)]
    pub(crate) deny: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SkillTools {
    #[serde(default)]
    pub(crate) config: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SkillHost {
    pub(crate) skill: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SkillWorkflow {
    pub(crate) profile: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SkillRouting {
    #[serde(default)]
    pub(crate) summary: Option<String>,
    #[serde(default)]
    pub(crate) triggers: Vec<String>,
    #[serde(default)]
    pub(crate) repo_markers: Vec<PathBuf>,
    #[serde(default)]
    pub(crate) exclude: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedSkill {
    pub(crate) manifest_path: PathBuf,
    pub(crate) manifest_dir: PathBuf,
    pub(crate) manifest: AirSkillManifest,
}

pub(crate) struct SkillRunOptions {
    pub(crate) reference: String,
    pub(crate) task: String,
    pub(crate) profile_override: Option<PathBuf>,
    pub(crate) model_config: Option<PathBuf>,
    pub(crate) trace_out: Option<PathBuf>,
    pub(crate) trace_redact: bool,
    pub(crate) trace_raw: bool,
    pub(crate) log: bool,
    pub(crate) tool_config_override: Option<PathBuf>,
    pub(crate) artifact_out: Option<PathBuf>,
    pub(crate) replay_artifact: Option<PathBuf>,
    pub(crate) replay_from: Option<usize>,
}

pub(crate) struct SkillAutoRunOptions {
    pub(crate) task: String,
    pub(crate) top_k: usize,
    pub(crate) profile_override: Option<PathBuf>,
    pub(crate) model_config: Option<PathBuf>,
    pub(crate) trace_out: Option<PathBuf>,
    pub(crate) trace_redact: bool,
    pub(crate) trace_raw: bool,
    pub(crate) log: bool,
    pub(crate) tool_config_override: Option<PathBuf>,
    pub(crate) artifact_out: Option<PathBuf>,
    pub(crate) replay_artifact: Option<PathBuf>,
    pub(crate) replay_from: Option<usize>,
}

pub(crate) struct SkillRouteOptions {
    pub(crate) task: String,
    pub(crate) top_k: usize,
    pub(crate) explain: bool,
}

pub(crate) struct PreparedSkillRun {
    pub(crate) metadata: CodeRunSkill,
    pub(crate) profile: PathBuf,
    pub(crate) tool_config: Option<PathBuf>,
    pub(crate) task_prefix: Option<String>,
    pub(crate) artifact_extra: BTreeMap<String, Value>,
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
    run: String,
    explain: String,
}

#[derive(Debug, Clone, Serialize)]
struct SkillRouteResult {
    selected: Vec<SkillRouteCard>,
    rejected: Vec<SkillRouteRejection>,
    candidates: Vec<SkillRouteCard>,
}

#[derive(Debug, Clone, Serialize)]
struct SkillRouteCard {
    id: String,
    mode: String,
    version: Option<String>,
    description: Option<String>,
    manifest: String,
    audit_risk: String,
    score: i32,
    reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct SkillRouteRejection {
    id: String,
    manifest: String,
    reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SkillAudit {
    pub(crate) schema: String,
    pub(crate) skill_id: String,
    pub(crate) risk: String,
    pub(crate) allowed_to_run: bool,
    pub(crate) findings: Vec<SkillAuditFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SkillAuditFinding {
    pub(crate) severity: String,
    pub(crate) reason: String,
    pub(crate) file: String,
}

pub(crate) fn list_skills() -> Result<()> {
    let skills = discover_skills()?
        .into_iter()
        .map(|skill| skill.list_item())
        .collect::<Result<Vec<_>>>()?;
    print_json(&json!({ "skills": skills }))
}

pub(crate) fn route_skill(options: SkillRouteOptions) -> Result<()> {
    let route = route_skills_for_task(&options.task, options.top_k.max(1))?;
    if options.explain {
        print_json(&json!({
            "task": options.task,
            "selected": route.selected,
            "rejected": route.rejected,
            "candidates": route.candidates,
        }))
    } else {
        print_json(&json!({
            "task": options.task,
            "selected": route.selected,
        }))
    }
}

fn route_skills_for_task(task: &str, top_k: usize) -> Result<SkillRouteResult> {
    let task_lower = task.to_ascii_lowercase();
    let mut selected = Vec::new();
    let mut rejected = Vec::new();
    let mut candidates = Vec::new();
    for skill in discover_skills()? {
        if skill.manifest.mode != SkillMode::Instruction {
            continue;
        }
        let audit = audit_resolved_skill(&skill)?;
        if !audit.allowed_to_run {
            rejected.push(SkillRouteRejection {
                id: skill.manifest.id.clone(),
                manifest: path_ref(&skill.manifest_path),
                reason: format!("audit risk {}", audit.risk),
            });
            continue;
        }
        let (score, reasons) = score_skill_for_task(&skill, &task_lower);
        let card = SkillRouteCard {
            id: skill.manifest.id.clone(),
            mode: skill.manifest.mode.as_str().to_string(),
            version: skill.manifest.version.clone(),
            description: skill_description_for_route(&skill),
            manifest: path_ref(&skill.manifest_path),
            audit_risk: audit.risk,
            score,
            reasons,
        };
        candidates.push(card.clone());
        if score > 0 {
            selected.push(card);
        }
    }
    candidates.sort_by(skill_route_order);
    selected.sort_by(skill_route_order);
    selected.truncate(top_k);
    Ok(SkillRouteResult {
        selected,
        rejected,
        candidates,
    })
}

fn skill_route_order(left: &SkillRouteCard, right: &SkillRouteCard) -> std::cmp::Ordering {
    right
        .score
        .cmp(&left.score)
        .then_with(|| left.id.cmp(&right.id))
}

fn score_skill_for_task(skill: &ResolvedSkill, task_lower: &str) -> (i32, Vec<String>) {
    let mut score = 0;
    let mut reasons = Vec::new();
    let routing = &skill.manifest.routing;
    for trigger in &routing.triggers {
        let trigger_lower = trigger.to_ascii_lowercase();
        if !trigger_lower.is_empty() && task_lower.contains(&trigger_lower) {
            score += 25;
            reasons.push(format!("task matches trigger `{trigger}`"));
        }
    }
    for excluded in &routing.exclude {
        let excluded_lower = excluded.to_ascii_lowercase();
        if !excluded_lower.is_empty() && task_lower.contains(&excluded_lower) {
            score -= 50;
            reasons.push(format!("task matches exclusion `{excluded}`"));
        }
    }
    if let Some(description) = &skill.manifest.description {
        for token in route_tokens(description) {
            if task_lower.contains(&token) {
                score += 3;
            }
        }
    }
    if let Some(summary) = &routing.summary {
        for token in route_tokens(summary) {
            if task_lower.contains(&token) {
                score += 5;
            }
        }
    }
    let matching_markers = routing
        .repo_markers
        .iter()
        .filter(|marker| marker.exists())
        .count();
    if matching_markers > 0 {
        score += (matching_markers as i32) * 8;
        reasons.push(format!("{matching_markers} repo marker(s) exist"));
    }
    if reasons.is_empty() && score > 0 {
        reasons.push("description matched task".to_string());
    }
    (score, reasons)
}

fn route_tokens(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_ascii_alphanumeric())
        .map(str::trim)
        .filter(|token| token.len() >= 4)
        .map(str::to_ascii_lowercase)
        .collect()
}

fn skill_description_for_route(skill: &ResolvedSkill) -> Option<String> {
    skill
        .manifest
        .routing
        .summary
        .clone()
        .or_else(|| skill.manifest.description.clone())
}

pub(crate) fn validate_skill(reference: &str) -> Result<()> {
    let skill = resolve_skill(reference)?;
    let checks = validate_resolved_skill(&skill)?;
    print_json(&json!({
        "valid": true,
        "skill": skill.manifest.id,
        "manifest": path_ref(&skill.manifest_path),
        "checks": checks,
    }))
}

pub(crate) fn explain_skill(reference: &str, profile_override: Option<PathBuf>) -> Result<()> {
    let skill = resolve_skill(reference)?;
    validate_resolved_skill(&skill)?;
    let effective = effective_execution(&skill)?;
    let profile = profile_override.unwrap_or_else(|| effective.profile.clone());
    let input = build_input("<task>".to_string());
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
    print_json(&explanation)
}

pub(crate) fn compile_skill(reference: &str, output: Option<PathBuf>) -> Result<()> {
    let skill = resolve_skill(reference)?;
    validate_resolved_skill(&skill)?;
    let effective = effective_execution(&skill)?;
    let compiled = json!({
        "schema": "air.compiled_skill.v1",
        "skill": skill_metadata(&skill)?,
        "mode": skill.manifest.mode.as_str(),
        "host": effective.host_skill,
        "manifest": path_ref(&skill.manifest_path),
        "profile": path_ref(&effective.profile),
        "tool_config": effective.tool_config.as_ref().map(|path| path_ref(path)),
        "instructions": skill.manifest.instructions.files.iter().map(|path| path_ref(&skill.manifest_dir.join(path))).collect::<Vec<_>>(),
        "effective_capabilities": effective.capabilities,
        "capabilities": {
            "allow": skill.manifest.capabilities.allow,
            "deny": skill.manifest.capabilities.deny,
        },
        "verification": skill.manifest.verification,
    });
    if let Some(output) = output {
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        fs::write(&output, serde_json::to_vec_pretty(&compiled)?)
            .with_context(|| format!("write {}", output.display()))?;
        print_json(&json!({ "compiled": path_ref(&output) }))
    } else {
        print_json(&compiled)
    }
}

pub(crate) fn run_skill(options: SkillRunOptions) -> Result<()> {
    let prepared = prepare_skill_run_context(&options.reference)?;
    let task = if let Some(prefix) = &prepared.task_prefix {
        format!("{prefix}\n\nTask: {}", options.task)
    } else {
        options.task.clone()
    };
    let outputs = run_code_agent(CodeOptions {
        task,
        artifact_task: Some(options.task.clone()),
        skill: Some(prepared.metadata.clone()),
        profile: Some(
            options
                .profile_override
                .unwrap_or_else(|| prepared.profile.clone()),
        ),
        model_config: options.model_config,
        trace_out: options.trace_out,
        trace_redact: options.trace_redact,
        trace_raw: options.trace_raw,
        log: options.log,
        tool_config: options
            .tool_config_override
            .or_else(|| prepared.tool_config.clone()),
        artifact_out: options.artifact_out,
        artifact_extra: prepared.artifact_extra,
        replay_artifact: options.replay_artifact,
        replay_from: options.replay_from,
    })?;
    print_json(&outputs)
}

pub(crate) fn run_skill_auto(options: SkillAutoRunOptions) -> Result<()> {
    let route = route_skills_for_task(&options.task, options.top_k.max(1))?;
    let selected = route
        .selected
        .first()
        .map(|card| card.id.clone())
        .unwrap_or_else(|| "code-agent".to_string());
    let prepared = prepare_skill_run_context(&selected)?;
    let task = if let Some(prefix) = &prepared.task_prefix {
        format!("{prefix}\n\nTask: {}", options.task)
    } else {
        options.task.clone()
    };
    let mut artifact_extra = prepared.artifact_extra.clone();
    artifact_extra.insert(
        "skill_route".to_string(),
        json!({
            "task": options.task,
            "selected_skill": selected,
            "selected": route.selected,
            "rejected": route.rejected,
        }),
    );
    let outputs = run_code_agent(CodeOptions {
        task,
        artifact_task: Some(options.task.clone()),
        skill: Some(prepared.metadata.clone()),
        profile: Some(
            options
                .profile_override
                .unwrap_or_else(|| prepared.profile.clone()),
        ),
        model_config: options.model_config,
        trace_out: options.trace_out,
        trace_redact: options.trace_redact,
        trace_raw: options.trace_raw,
        log: options.log,
        tool_config: options
            .tool_config_override
            .or_else(|| prepared.tool_config.clone()),
        artifact_out: options.artifact_out,
        artifact_extra,
        replay_artifact: options.replay_artifact,
        replay_from: options.replay_from,
    })?;
    print_json(&outputs)
}

pub(crate) fn prepare_skill_run_context(reference: &str) -> Result<PreparedSkillRun> {
    let skill = resolve_skill(reference)?;
    validate_resolved_skill(&skill)?;
    let audit = audit_resolved_skill(&skill)?;
    if !audit.allowed_to_run {
        bail!(
            "skill `{}` audit risk is {}; refusing to run",
            skill.manifest.id,
            audit.risk
        );
    }
    let effective = effective_execution(&skill)?;
    let metadata = skill_metadata_with_context(&skill, Some(&audit), &effective)?;
    let preloaded_skills = preload_skill_for_run(&skill, &audit)?;
    let task_prefix = if preloaded_skills.is_empty() {
        None
    } else {
        Some(render_preloaded_skill_instructions(&preloaded_skills))
    };
    let artifact_extra = BTreeMap::from([
        ("skill".to_string(), json!(metadata.clone())),
        (
            "skill_effective_execution".to_string(),
            json!({
                "mode": skill.manifest.mode.as_str(),
                "host": effective.host_skill,
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

pub(crate) fn audit_skill(reference: &str, write: bool) -> Result<()> {
    let skill = resolve_skill(reference)?;
    let audit = audit_resolved_skill(&skill)?;
    if write {
        let path = skill.manifest_dir.join("audit.json");
        fs::write(&path, serde_json::to_vec_pretty(&audit)?)
            .with_context(|| format!("write {}", path.display()))?;
    }
    print_json(&json!(audit))
}

pub(crate) fn import_skill(source: &str, out: &Path) -> Result<()> {
    if out.exists() {
        bail!("skill import output already exists: {}", out.display());
    }
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
    fs::create_dir_all(out.parent().unwrap_or_else(|| Path::new(".")))
        .with_context(|| format!("create parent for {}", out.display()))?;
    copy_dir_all(&imported_root, out)?;
    ensure_import_manifest(source, out)?;
    let resolved = resolve_skill(out.to_string_lossy().as_ref())?;
    let audit = audit_resolved_skill(&resolved)?;
    fs::write(out.join("audit.json"), serde_json::to_vec_pretty(&audit)?)
        .with_context(|| format!("write {}", out.join("audit.json").display()))?;
    fs::write(
        out.join("source.json"),
        serde_json::to_vec_pretty(&json!({
            "source": source,
            "imported_at_unix_ms": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
            "manifest": path_ref(&resolved.manifest_path),
            "sha256": directory_identity(out)?,
        }))?,
    )
    .with_context(|| format!("write {}", out.join("source.json").display()))?;
    let _ = fs::remove_dir_all(temp_dir);
    print_json(&json!({
        "imported": path_ref(out),
        "skill": resolved.manifest.id,
        "audit_risk": audit.risk,
    }))
}

pub(crate) fn resolve_skill(reference: &str) -> Result<ResolvedSkill> {
    let path = PathBuf::from(reference);
    let manifest_path = if path.exists() {
        manifest_path_for_existing_path(&path)?
    } else {
        find_skill_manifest_by_id(reference)
            .with_context(|| format!("unknown AIR skill `{reference}`; run `air skill list`"))?
    };
    read_skill_manifest(&manifest_path)
}

pub(crate) fn resolve_skill_run_metadata(reference: &str) -> Result<CodeRunSkill> {
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
    let mut findings = Vec::new();
    for path in skill_files(&skill.manifest_dir)? {
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => continue,
        };
        let relative = path_ref(path.strip_prefix(&skill.manifest_dir).unwrap_or(&path));
        let lower = content.to_ascii_lowercase();
        for (severity, reason, needles) in audit_patterns() {
            if needles.iter().any(|needle| lower.contains(needle)) {
                findings.push(SkillAuditFinding {
                    severity: severity.to_string(),
                    reason: reason.to_string(),
                    file: relative.clone(),
                });
            }
        }
        if is_executable_script_path(&path) {
            findings.push(SkillAuditFinding {
                severity: "medium".to_string(),
                reason: "skill contains executable script content".to_string(),
                file: relative,
            });
        }
    }
    for capability in &skill.manifest.capabilities.allow {
        if matches!(
            capability.as_str(),
            "shell" | "shell.unrestricted" | "network" | "secrets.read"
        ) {
            findings.push(SkillAuditFinding {
                severity: "high".to_string(),
                reason: format!("skill requests high-risk capability `{capability}`"),
                file: path_ref(&skill.manifest_path),
            });
        }
    }
    let risk = audit_risk(&findings);
    Ok(SkillAudit {
        schema: "air.skill_audit.v1".to_string(),
        skill_id: skill.manifest.id.clone(),
        allowed_to_run: !matches!(risk.as_str(), "critical" | "high"),
        risk,
        findings,
    })
}

fn audit_patterns() -> Vec<(&'static str, &'static str, Vec<&'static str>)> {
    vec![
        (
            "critical",
            "remote shell execution pattern",
            vec![
                "curl | sh",
                "curl -fs",
                "wget | sh",
                "iex(",
                "powershell -enc",
            ],
        ),
        (
            "critical",
            "secret or wallet path access pattern",
            vec![
                "~/.ssh",
                ".aws/credentials",
                "login keychain",
                "metamask",
                "wallet.dat",
            ],
        ),
        (
            "high",
            "environment or credential exfiltration pattern",
            vec![
                "process.env",
                "os.environ",
                "printenv",
                "cat .env",
                "env |",
                "dotenv",
            ],
        ),
        (
            "high",
            "dynamic command execution pattern",
            vec!["bash -c", "sh -c", "python -c", "node -e", "eval "],
        ),
    ]
}

fn audit_risk(findings: &[SkillAuditFinding]) -> String {
    if findings
        .iter()
        .any(|finding| finding.severity == "critical")
    {
        "critical".to_string()
    } else if findings.iter().any(|finding| finding.severity == "high") {
        "high".to_string()
    } else if findings.iter().any(|finding| finding.severity == "medium") {
        "medium".to_string()
    } else {
        "low".to_string()
    }
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
    let id = out
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("imported-skill")
        .to_string();
    let description = format!("Imported AIR skill wrapper for {source}");
    let manifest = AirSkillManifest {
        schema: Some(SKILL_SCHEMA.to_string()),
        id,
        mode: SkillMode::Instruction,
        version: Some("0.1.0".to_string()),
        description: Some(description),
        source: Some(json!({
            "type": "imported",
            "original": source,
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
        routing: SkillRouting::default(),
        verification: json!({
            "mode": "instruction_only",
            "scripts": "disabled",
        }),
    };
    fs::write(&manifest_path, serde_yaml::to_string(&manifest)?)
        .with_context(|| format!("write {}", manifest_path.display()))
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
        crate::code_artifact::sha256_hex(&encoded)
    ))
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
    let profile_config = read_run_plan_profile(&profile.to_path_buf())?;
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
        "instructions": load_instruction_files(skill, 65_536)?,
    })])
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
            description: self.manifest.description.clone(),
            manifest: path_ref(&self.manifest_path),
            profile: path_ref(&effective.profile),
            tool_config: effective.tool_config.map(|path| path_ref(&path)),
            host: self.host_skill_id().map(str::to_string),
            run: format!("air skill run {} \"<task>\"", self.manifest.id),
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
        assert!(route.selected.iter().any(|card| card.id == "tdd-workflow"));
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
