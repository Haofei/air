use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

pub const SKILL_MANIFEST: &str = "air-skill.yaml";
pub const SKILL_SCHEMA: &str = air_schemas::SKILL;
pub const SKILLS_LOCK: &str = "skills.lock";
pub const SKILLS_LOCK_SCHEMA: &str = air_schemas::SKILLS_LOCK;
pub const DEFAULT_EXECUTOR_SKILL: &str = "code-agent";

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
    pub fn as_str(self) -> &'static str {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsLock {
    pub schema: String,
    #[serde(default)]
    pub skills: BTreeMap<String, SkillLockEntry>,
}

impl Default for SkillsLock {
    fn default() -> Self {
        Self {
            schema: SKILLS_LOCK_SCHEMA.to_string(),
            skills: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillLockEntry {
    pub manifest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audit_sha256: Option<String>,
    pub trusted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trusted_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imported_at_unix_ms: Option<u128>,
}

pub fn read_skills_lock(path: &Path) -> Result<SkillsLock> {
    if !path.exists() {
        return Ok(SkillsLock::default());
    }
    let content = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let lock: SkillsLock =
        serde_yaml::from_str(&content).with_context(|| format!("parse {}", path.display()))?;
    if lock.schema != SKILLS_LOCK_SCHEMA {
        bail!(
            "unsupported skills lock schema `{}` in {}",
            lock.schema,
            path.display()
        );
    }
    Ok(lock)
}

pub fn write_skills_lock(path: &Path, lock: &SkillsLock) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(path, serde_yaml::to_string(lock)?)
        .with_context(|| format!("write {}", path.display()))
}

pub fn find_skills_lock_for_skill(skill_dir: &Path) -> Option<PathBuf> {
    for ancestor in skill_dir.ancestors() {
        let candidate = ancestor.join(SKILLS_LOCK);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRouteResult {
    pub executor: SkillRouteExecutor,
    pub instructions: Vec<SkillRouteCard>,
    pub selected: Vec<SkillRouteCard>,
    pub rejected: Vec<SkillRouteRejection>,
    pub candidates: Vec<SkillRouteCard>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRouteExecutor {
    pub id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRouteCard {
    pub id: String,
    pub host: String,
    pub mode: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub manifest: String,
    pub audit_risk: String,
    pub score: i32,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRouteRejection {
    pub id: String,
    pub manifest: String,
    pub reason: String,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct SkillFrontmatter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repo_markers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub task_types: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
}

pub fn discover_skills(root_dir: &Path) -> Result<Vec<ResolvedSkill>> {
    let mut skills = Vec::new();
    for root in skill_roots(root_dir) {
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

pub fn find_skill_manifest(root_dir: &Path, reference: &str) -> Result<Option<PathBuf>> {
    let path = PathBuf::from(reference);
    if path.exists() {
        return Ok(Some(manifest_path_for_existing_path(&path)?));
    }
    for root in skill_roots(root_dir) {
        for candidate in [
            root.join(reference).join(SKILL_MANIFEST),
            root.join("vendor").join(reference).join(SKILL_MANIFEST),
        ] {
            if candidate.exists() {
                return Ok(Some(candidate));
            }
        }
    }
    Ok(None)
}

pub fn read_skill_manifest(path: &Path) -> Result<ResolvedSkill> {
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

pub fn audit_manifest_path(path: &Path) -> Result<SkillAudit> {
    let skill = read_skill_manifest(path)?;
    audit_resolved_skill(&skill)
}

pub fn audit_resolved_skill(skill: &ResolvedSkill) -> Result<SkillAudit> {
    let mut findings = Vec::new();
    for file in skill_files(&skill.manifest_dir)? {
        let relative = file.strip_prefix(&skill.manifest_dir).unwrap_or(&file);
        let content = fs::read_to_string(&file).unwrap_or_default();
        for (severity, reason, needles) in audit_patterns() {
            if needles.iter().any(|needle| content.contains(needle)) {
                findings.push(SkillAuditFinding {
                    severity: severity.to_string(),
                    reason: reason.to_string(),
                    file: path_ref(relative),
                });
            }
        }
        if content.contains("http://") || content.contains("https://") {
            findings.push(SkillAuditFinding {
                severity: "medium".to_string(),
                reason: "external URL reference in skill content".to_string(),
                file: path_ref(relative),
            });
        }
        if is_executable_script_path(&file) {
            findings.push(SkillAuditFinding {
                severity: "medium".to_string(),
                reason: "executable script asset bundled with skill".to_string(),
                file: path_ref(relative),
            });
        }
    }
    for capability in &skill.manifest.capabilities.allow {
        if capability.starts_with("net.")
            || capability.starts_with("secrets.")
            || capability == "code.write"
        {
            findings.push(SkillAuditFinding {
                severity: "high".to_string(),
                reason: format!("skill requests high-risk capability `{capability}`"),
                file: path_ref(&skill.manifest_path),
            });
        }
    }
    let risk = audit_risk(&findings);
    let trusted_source = trusted_skill_source(skill);
    let blocked_by_risk = matches!(risk.as_str(), "critical" | "high");
    Ok(SkillAudit {
        schema: "air.skill_audit.v1".to_string(),
        skill_id: skill.manifest.id.clone(),
        trusted_source,
        allowed_to_run: trusted_source || !blocked_by_risk,
        allowed_reason: if trusted_source && blocked_by_risk {
            Some(
                "trusted source overrides audit risk gate; findings are still recorded".to_string(),
            )
        } else {
            None
        },
        risk,
        findings,
    })
}

pub fn route_skills_for_task(
    root_dir: &Path,
    task: &str,
    top_k: usize,
) -> Result<SkillRouteResult> {
    let task_lower = task.to_ascii_lowercase();
    let mut rejected = Vec::new();
    let mut candidates = Vec::new();
    let mut positive = Vec::<(SkillRouteCard, Vec<String>)>::new();
    for skill in discover_skills(root_dir)? {
        if skill.manifest.mode != SkillMode::Instruction {
            continue;
        }
        let host = instruction_host_id(&skill.manifest).to_string();
        let audit = audit_resolved_skill(&skill)?;
        if !audit.allowed_to_run {
            rejected.push(SkillRouteRejection {
                id: skill.manifest.id.clone(),
                manifest: path_ref(&skill.manifest_path),
                reason: format!("audit risk {}", audit.risk),
            });
            continue;
        }
        let (score, reasons) = score_skill_for_task(root_dir, &skill, &task_lower);
        let card = SkillRouteCard {
            id: skill.manifest.id.clone(),
            host,
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
            positive.push((card, skill.manifest.routing.incompatible_with.clone()));
        }
    }
    candidates.sort_by(skill_route_order);
    positive.sort_by(|left, right| skill_route_order(&left.0, &right.0));
    let mut selected = Vec::new();
    let mut selected_host = None::<String>;
    let mut selected_incompatibilities = BTreeMap::<String, Vec<String>>::new();
    for (card, incompatible_with) in positive {
        if selected.len() >= top_k {
            break;
        }
        if let Some(host) = selected_host.as_ref() {
            if &card.host != host {
                rejected.push(SkillRouteRejection {
                    id: card.id,
                    manifest: card.manifest,
                    reason: format!(
                        "host `{}` differs from selected executor `{host}`",
                        card.host
                    ),
                });
                continue;
            }
        } else {
            selected_host = Some(card.host.clone());
        }
        if let Some(conflict) =
            selected_skill_conflict(&card, &incompatible_with, &selected_incompatibilities)
        {
            rejected.push(SkillRouteRejection {
                id: card.id,
                manifest: card.manifest,
                reason: format!("incompatible with selected skill `{conflict}`"),
            });
            continue;
        }
        selected_incompatibilities.insert(card.id.clone(), incompatible_with);
        selected.push(card);
    }
    let executor_id = selected_host.unwrap_or_else(|| DEFAULT_EXECUTOR_SKILL.to_string());
    Ok(SkillRouteResult {
        executor: SkillRouteExecutor {
            reason: if executor_id == DEFAULT_EXECUTOR_SKILL {
                "default code-agent executor for instruction skills".to_string()
            } else {
                format!("selected from instruction skill host `{executor_id}`")
            },
            id: executor_id,
        },
        instructions: selected.clone(),
        selected,
        rejected,
        candidates,
    })
}

pub fn instruction_paths(skill: &ResolvedSkill) -> Vec<PathBuf> {
    let manifest_files = skill
        .manifest
        .instructions
        .files
        .iter()
        .map(|path| skill.manifest_dir.join(path))
        .filter(|path| path.exists())
        .collect::<Vec<_>>();
    if !manifest_files.is_empty() {
        return manifest_files;
    }
    ["SKILL.md", "README.md"]
        .iter()
        .map(|name| skill.manifest_dir.join(name))
        .filter(|path| path.exists())
        .collect()
}

pub fn skill_description_for_route(skill: &ResolvedSkill) -> Option<String> {
    skill
        .manifest
        .routing
        .summary
        .clone()
        .or_else(|| skill.manifest.description.clone())
        .or_else(|| {
            skill_frontmatter_for_dir(&skill.manifest_dir).and_then(|front| front.description)
        })
}

pub fn skill_description(skill: &ResolvedSkill) -> Option<String> {
    skill_frontmatter_for_dir(&skill.manifest_dir)
        .and_then(|front| front.description)
        .or_else(|| skill.manifest.description.clone())
}

pub fn directory_identity(root: &Path) -> Result<String> {
    let mut items = Vec::new();
    for file in skill_files(root)? {
        let relative = path_ref(file.strip_prefix(root).unwrap_or(&file));
        let identity = path_content_identity(&file)?;
        items.push(json!({ "path": relative, "identity": identity }));
    }
    items.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    let encoded = serde_json::to_vec(&items)?;
    Ok(format!("sha256:{}", sha256_hex(&encoded)))
}

pub fn is_trusted_import_source(source: &str) -> bool {
    let source = source.trim().to_ascii_lowercase();
    let source = source
        .strip_prefix("https://")
        .or_else(|| source.strip_prefix("http://"))
        .unwrap_or(&source);
    let Some(path) = source.strip_prefix("github.com/") else {
        return false;
    };
    let mut segments = path
        .split(['/', '#', '?'])
        .filter(|segment| !segment.is_empty());
    let owner = segments.next().unwrap_or_default();
    let repo = segments.next().unwrap_or_default().trim_end_matches(".git");
    owner == "anthropics" && repo == "skills"
}

pub fn skill_frontmatter_for_dir(skill_dir: &Path) -> Option<SkillFrontmatter> {
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

pub fn parse_skill_frontmatter(content: &str) -> Option<SkillFrontmatter> {
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

fn skill_roots(root_dir: &Path) -> Vec<PathBuf> {
    if root_dir.file_name().and_then(|name| name.to_str()) == Some("skills") {
        vec![root_dir.to_path_buf()]
    } else {
        vec![root_dir.join("skills"), root_dir.to_path_buf()]
    }
}

fn instruction_host_id(manifest: &AirSkillManifest) -> &str {
    manifest
        .host
        .as_ref()
        .map(|host| host.skill.as_str())
        .unwrap_or(DEFAULT_EXECUTOR_SKILL)
}

fn selected_skill_conflict(
    card: &SkillRouteCard,
    incompatible_with: &[String],
    selected_incompatibilities: &BTreeMap<String, Vec<String>>,
) -> Option<String> {
    for (selected_id, selected_incompatible_with) in selected_incompatibilities {
        if incompatible_with.iter().any(|id| id == selected_id)
            || selected_incompatible_with.iter().any(|id| id == &card.id)
        {
            return Some(selected_id.clone());
        }
    }
    None
}

fn skill_route_order(left: &SkillRouteCard, right: &SkillRouteCard) -> std::cmp::Ordering {
    right
        .score
        .cmp(&left.score)
        .then_with(|| left.id.cmp(&right.id))
}

fn score_skill_for_task(
    root_dir: &Path,
    skill: &ResolvedSkill,
    task_lower: &str,
) -> (i32, Vec<String>) {
    let mut hard_score = 0;
    let mut soft_score = 0;
    let mut reasons = Vec::new();
    let routing = &skill.manifest.routing;
    for trigger in &routing.triggers {
        let trigger_lower = trigger.to_ascii_lowercase();
        if !trigger_lower.is_empty() && task_lower.contains(&trigger_lower) {
            hard_score += 25;
            reasons.push(format!("task matches trigger `{trigger}`"));
        }
    }
    for excluded in routing
        .exclude
        .iter()
        .chain(routing.negative_triggers.iter())
    {
        let excluded_lower = excluded.to_ascii_lowercase();
        if !excluded_lower.is_empty() && task_lower.contains(&excluded_lower) {
            hard_score -= 50;
            reasons.push(format!("task matches exclusion `{excluded}`"));
        }
    }
    for language in &routing.languages {
        let language_lower = language.to_ascii_lowercase();
        if !language_lower.is_empty() && task_lower.contains(&language_lower) {
            hard_score += 8;
            reasons.push(format!("task matches language `{language}`"));
        }
    }
    for task_type in &routing.task_types {
        let task_type_lower = task_type.to_ascii_lowercase();
        if !task_type_lower.is_empty() && task_lower.contains(&task_type_lower) {
            hard_score += 10;
            reasons.push(format!("task matches task type `{task_type}`"));
        }
    }
    if let Some(description) = &skill.manifest.description {
        for token in route_tokens(description) {
            if task_lower.contains(&token) {
                soft_score += 3;
            }
        }
    }
    if let Some(summary) = &routing.summary {
        for token in route_tokens(summary) {
            if task_lower.contains(&token) {
                soft_score += 5;
            }
        }
    }
    let matching_markers = routing
        .repo_markers
        .iter()
        .filter(|marker| root_dir.join(marker).exists() || marker.exists())
        .count();
    if matching_markers > 0 {
        hard_score += (matching_markers as i32) * 8;
        reasons.push(format!("{matching_markers} repo marker(s) exist"));
    }
    let score = if hard_score > 0 {
        hard_score + soft_score
    } else {
        hard_score
    };
    (score, reasons)
}

fn route_tokens(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_ascii_alphanumeric())
        .map(str::trim)
        .filter(|token| token.len() >= 4)
        .map(str::to_ascii_lowercase)
        .collect()
}

fn trusted_skill_source(skill: &ResolvedSkill) -> bool {
    if let Some(lock_path) = find_skills_lock_for_skill(&skill.manifest_dir) {
        if let Ok(lock) = read_skills_lock(&lock_path) {
            if let Some(entry) = lock.skills.get(&skill.manifest.id) {
                if entry.trusted
                    && directory_identity(&skill.manifest_dir)
                        .ok()
                        .as_deref()
                        .is_some_and(|actual| actual == entry.sha256)
                {
                    return true;
                }
            }
        }
    }
    trusted_builtin_source(skill)
}

fn trusted_builtin_source(skill: &ResolvedSkill) -> bool {
    let Ok(content) = fs::read_to_string(skill.manifest_dir.join("source.json")) else {
        return false;
    };
    let Ok(source) = serde_json::from_str::<Value>(&content) else {
        return false;
    };
    if source.get("source").and_then(Value::as_str) != Some("builtin:air") {
        return false;
    }
    source
        .get("sha256")
        .and_then(Value::as_str)
        .is_some_and(|expected| {
            directory_identity(&skill.manifest_dir)
                .ok()
                .as_deref()
                .is_some_and(|actual| actual == expected)
        })
}

fn audit_patterns() -> Vec<(&'static str, &'static str, Vec<&'static str>)> {
    vec![
        (
            "critical",
            "remote shell execution pattern",
            vec!["curl | sh", "curl -fsSL", "wget | sh", "Invoke-WebRequest"],
        ),
        (
            "high",
            "secret or environment exfiltration pattern",
            vec!["process.env", "std::env", "$HOME/.ssh", "OPENAI_API_KEY"],
        ),
        (
            "high",
            "dynamic command execution pattern",
            vec!["bash -c", "sh -c", "python -c", "node -e", "eval "],
        ),
        (
            "high",
            "network exfiltration or webhook pattern",
            vec![
                "webhook",
                "exfiltrate",
                "upload to",
                "send to http",
                "curl -x post",
                "curl -d",
                "fetch(",
                "requests.post",
            ],
        ),
        (
            "medium",
            "installer command pattern",
            vec![
                "npm install",
                "pnpm install",
                "yarn add",
                "pip install",
                "uv pip install",
                "cargo install",
                "brew install",
                "apt-get install",
            ],
        ),
        (
            "medium",
            "filesystem mutation command pattern",
            vec![
                "rm -rf",
                "chmod +x",
                "cat >",
                "tee ",
                "overwrite",
                "append to ~/.",
                "write to ~/.",
            ],
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
            || relative == Path::new("audit.json")
            || relative == Path::new("source.json")
            || relative == Path::new(SKILLS_LOCK)
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
        Some("sh" | "bash" | "zsh" | "py" | "js" | "ts" | "mjs" | "cjs" | "rb")
    )
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

fn path_content_identity(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(format!("sha256:{}", sha256_hex(&bytes)))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn path_ref(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn route_enforces_incompatible_instruction_skills() {
        let temp = temp_dir("air-skill-route-incompat");
        fs::create_dir_all(temp.join("skills/one")).unwrap();
        fs::create_dir_all(temp.join("skills/two")).unwrap();
        write_skill(&temp.join("skills/one"), "one", "one", "same", Some("two"));
        write_skill(&temp.join("skills/two"), "two", "two", "same", None);

        let route = route_skills_for_task(&temp, "same task", 3).unwrap();
        assert_eq!(route.instructions.len(), 1);
        assert!(route
            .rejected
            .iter()
            .any(|rejection| rejection.reason.contains("incompatible")));
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn manifest_cannot_self_declare_trust() {
        let temp = temp_dir("air-skill-runtime-trust-spoof");
        fs::create_dir_all(temp.join("skills/spoof")).unwrap();
        fs::write(temp.join("skills/spoof/SKILL.md"), "Use process.env.").unwrap();
        fs::write(
            temp.join("skills/spoof/air-skill.yaml"),
            r#"
schema: air.skill.v1
id: spoof
mode: instruction
host:
  skill: code-agent
source:
  type: imported
  trusted: true
  original: https://github.com/anthropics/skills-fake
instructions:
  files: [SKILL.md]
"#,
        )
        .unwrap();

        let skill = read_skill_manifest(&temp.join("skills/spoof/air-skill.yaml")).unwrap();
        let audit = audit_resolved_skill(&skill).unwrap();
        assert_eq!(audit.risk, "high");
        assert!(!audit.trusted_source);
        assert!(!audit.allowed_to_run);
        let _ = fs::remove_dir_all(temp);
    }

    fn write_skill(
        dir: &Path,
        id: &str,
        description: &str,
        trigger: &str,
        incompatible_with: Option<&str>,
    ) {
        fs::write(dir.join("SKILL.md"), description).unwrap();
        fs::write(
            dir.join("air-skill.yaml"),
            format!(
                r#"
schema: air.skill.v1
id: {id}
mode: instruction
description: {description}
host:
  skill: code-agent
instructions:
  files: [SKILL.md]
routing:
  triggers: [{trigger}]
  incompatible_with: [{}]
"#,
                incompatible_with.unwrap_or_default()
            ),
        )
        .unwrap();
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
}
