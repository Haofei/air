use air_runtime::RuntimeError;
use air_tools_core::text::bytes_to_limited_text;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const SKILL_MANIFEST: &str = "air-skill.yaml";

#[derive(Debug, Deserialize)]
struct SkillManifest {
    id: String,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    instructions: SkillInstructions,
    #[serde(default)]
    host: Option<SkillHost>,
    #[serde(default)]
    routing: SkillRouting,
}

#[derive(Debug, Default, Deserialize)]
struct SkillHost {
    skill: String,
}

#[derive(Debug, Default, Deserialize)]
struct SkillInstructions {
    #[serde(default)]
    files: Vec<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
struct SkillRouting {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    triggers: Vec<String>,
    #[serde(default)]
    negative_triggers: Vec<String>,
    #[serde(default)]
    repo_markers: Vec<PathBuf>,
    #[serde(default)]
    languages: Vec<String>,
    #[serde(default)]
    task_types: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
}

pub(crate) fn call_skill_tool(
    name: &str,
    input: &Value,
    root_dir: &Path,
    max_bytes: usize,
) -> Result<Value, RuntimeError> {
    let root_dir = root_dir
        .canonicalize()
        .unwrap_or_else(|_| root_dir.to_path_buf());
    if let Some(task) = input
        .get("route_task")
        .or_else(|| input.get("task"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let top_k = input
            .get("top_k")
            .or_else(|| input.get("topK"))
            .and_then(Value::as_u64)
            .unwrap_or(3)
            .max(1) as usize;
        return route_skills(&root_dir, task, top_k);
    }
    let skill_name = input
        .get("name")
        .or_else(|| input.get("skill"))
        .or_else(|| input.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match skill_name {
        Some(skill_name) => load_skill(name, &root_dir, skill_name, max_bytes),
        None => list_skills(&root_dir),
    }
}

fn route_skills(root_dir: &Path, task: &str, top_k: usize) -> Result<Value, RuntimeError> {
    let task_lower = task.to_ascii_lowercase();
    let mut selected = Vec::new();
    let mut rejected = Vec::new();
    let mut candidates = Vec::new();
    for manifest_path in discover_skill_manifests(root_dir)? {
        let manifest = match read_manifest(&manifest_path) {
            Ok(manifest) => manifest,
            Err(_) => continue,
        };
        if manifest.mode.as_deref().unwrap_or("executor") != "instruction" {
            continue;
        }
        let host = manifest
            .host
            .as_ref()
            .map(|host| host.skill.as_str())
            .unwrap_or("code-agent");
        if host != "code-agent" {
            rejected.push(json!({
                "id": manifest.id,
                "manifest": display_path(&manifest_path),
                "reason": format!("instruction skill host `{host}` is not `code-agent`"),
            }));
            continue;
        }
        let risk = read_audit_risk(&manifest_path).unwrap_or_else(|| "unknown".to_string());
        if matches!(risk.as_str(), "high" | "critical") {
            rejected.push(json!({
                "id": manifest.id,
                "manifest": display_path(&manifest_path),
                "reason": format!("audit risk {risk}"),
            }));
            continue;
        }
        let skill_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
        let (score, reasons) = score_skill_for_task(root_dir, &manifest, &task_lower);
        let card = json!({
            "id": manifest.id.clone(),
            "mode": manifest.mode.clone().unwrap_or_else(|| "executor".to_string()),
            "version": manifest.version.clone(),
            "description": skill_description(skill_dir, &manifest),
            "manifest": display_path(&manifest_path),
            "audit_risk": risk,
            "score": score,
            "reasons": reasons,
        });
        candidates.push(card.clone());
        if score > 0 {
            selected.push(card);
        }
    }
    sort_route_cards(&mut selected);
    sort_route_cards(&mut candidates);
    selected.truncate(top_k);
    Ok(json!({
        "kind": "skill_route",
        "task": task,
        "executor": {
            "id": "code-agent",
            "reason": "default code-agent executor for instruction skills"
        },
        "instructions": selected.clone(),
        "skills": selected,
        "rejected": rejected,
        "candidates": candidates,
    }))
}

fn sort_route_cards(cards: &mut [Value]) {
    cards.sort_by(|left, right| {
        let left_score = left.get("score").and_then(Value::as_i64).unwrap_or(0);
        let right_score = right.get("score").and_then(Value::as_i64).unwrap_or(0);
        let left_id = left.get("id").and_then(Value::as_str).unwrap_or("");
        let right_id = right.get("id").and_then(Value::as_str).unwrap_or("");
        right_score
            .cmp(&left_score)
            .then_with(|| left_id.cmp(right_id))
    });
}

fn score_skill_for_task(
    root_dir: &Path,
    manifest: &SkillManifest,
    task_lower: &str,
) -> (i32, Vec<String>) {
    let mut score = 0;
    let mut reasons = Vec::new();
    for trigger in &manifest.routing.triggers {
        let trigger_lower = trigger.to_ascii_lowercase();
        if !trigger_lower.is_empty() && task_lower.contains(&trigger_lower) {
            score += 25;
            reasons.push(format!("task matches trigger `{trigger}`"));
        }
    }
    for excluded in manifest
        .routing
        .exclude
        .iter()
        .chain(manifest.routing.negative_triggers.iter())
    {
        let excluded_lower = excluded.to_ascii_lowercase();
        if !excluded_lower.is_empty() && task_lower.contains(&excluded_lower) {
            score -= 50;
            reasons.push(format!("task matches exclusion `{excluded}`"));
        }
    }
    for language in &manifest.routing.languages {
        let language_lower = language.to_ascii_lowercase();
        if !language_lower.is_empty() && task_lower.contains(&language_lower) {
            score += 8;
            reasons.push(format!("task matches language `{language}`"));
        }
    }
    for task_type in &manifest.routing.task_types {
        let task_type_lower = task_type.to_ascii_lowercase();
        if !task_type_lower.is_empty() && task_lower.contains(&task_type_lower) {
            score += 10;
            reasons.push(format!("task matches task type `{task_type}`"));
        }
    }
    for token in manifest
        .description
        .iter()
        .flat_map(|description| route_tokens(description))
    {
        if task_lower.contains(&token) {
            score += 3;
        }
    }
    for token in manifest
        .routing
        .summary
        .iter()
        .flat_map(|summary| route_tokens(summary))
    {
        if task_lower.contains(&token) {
            score += 5;
        }
    }
    let markers = manifest
        .routing
        .repo_markers
        .iter()
        .filter(|marker| root_dir.join(marker).exists() || marker.exists())
        .count();
    if markers > 0 {
        score += (markers as i32) * 8;
        reasons.push(format!("{markers} repo marker(s) exist"));
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

fn list_skills(root_dir: &Path) -> Result<Value, RuntimeError> {
    let skills = discover_skill_manifests(root_dir)?
        .into_iter()
        .filter_map(|manifest_path| {
            read_manifest(&manifest_path)
                .ok()
                .map(|manifest| (manifest_path, manifest))
        })
        .map(|(manifest_path, manifest)| {
            let skill_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
            json!({
                "id": manifest.id,
                "version": manifest.version,
                "description": skill_description(skill_dir, &manifest),
                "manifest": display_path(&manifest_path),
                "audit_risk": read_audit_risk(&manifest_path),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "kind": "skill_list",
        "skills": skills,
    }))
}

fn load_skill(
    tool_name: &str,
    root_dir: &Path,
    skill_name: &str,
    max_bytes: usize,
) -> Result<Value, RuntimeError> {
    let manifest_path = find_skill_manifest(root_dir, skill_name)?.ok_or_else(|| {
        RuntimeError::Provider(format!(
            "tool {tool_name} could not find AIR skill `{skill_name}`"
        ))
    })?;
    let manifest = read_manifest(&manifest_path)?;
    if let Some(risk) = read_audit_risk(&manifest_path) {
        if matches!(risk.as_str(), "high" | "critical") {
            return Err(RuntimeError::Provider(format!(
                "tool {tool_name} refused to load AIR skill `{skill_name}` because audit risk is {risk}"
            )));
        }
    }
    let skill_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let instruction_paths = instruction_paths(skill_dir, &manifest);
    let mut remaining = max_bytes;
    let mut instructions = Vec::new();
    let mut artifacts = Vec::new();
    for path in instruction_paths {
        let bytes = fs::read(&path).map_err(|error| {
            RuntimeError::Provider(format!(
                "tool {tool_name} failed to read skill instruction {}: {error}",
                path.display()
            ))
        })?;
        let budget = remaining.min(bytes.len()).max(remaining.min(1));
        let (content, truncated, bytes_returned) = bytes_to_limited_text(&bytes, budget);
        remaining = remaining.saturating_sub(bytes_returned);
        let path_ref = display_path(&path);
        instructions.push(json!({
            "path": path_ref,
            "content": content,
            "truncated": truncated || bytes_returned < bytes.len(),
            "bytes": bytes.len(),
            "bytes_returned": bytes_returned,
        }));
        artifacts.push(json!({
            "id": format!("skill:{}:{}", manifest.id, path.file_name().and_then(|name| name.to_str()).unwrap_or("instruction")),
            "kind": "skill_instruction",
            "title": format!("{} instruction", manifest.id),
            "uri": path_ref,
            "content": "",
            "metadata": {
                "provider": "skill",
                "skill": manifest.id,
            }
        }));
        if remaining == 0 {
            break;
        }
    }
    Ok(json!({
        "kind": "skill_instruction",
        "skill": {
            "id": manifest.id,
            "version": manifest.version,
            "description": manifest.description,
            "manifest": display_path(&manifest_path),
            "audit_risk": read_audit_risk(&manifest_path),
        },
        "instructions": instructions,
        "artifacts": artifacts,
    }))
}

fn discover_skill_manifests(root_dir: &Path) -> Result<Vec<PathBuf>, RuntimeError> {
    let mut manifests = Vec::new();
    let mut seen = BTreeSet::new();
    for base in skill_search_roots(root_dir) {
        if !base.exists() {
            continue;
        }
        collect_child_manifests(&base, &mut seen, &mut manifests)?;
        collect_child_manifests(&base.join("vendor"), &mut seen, &mut manifests)?;
    }
    Ok(manifests)
}

fn collect_child_manifests(
    base: &Path,
    seen: &mut BTreeSet<PathBuf>,
    manifests: &mut Vec<PathBuf>,
) -> Result<(), RuntimeError> {
    if !base.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(base).map_err(|error| {
        RuntimeError::Provider(format!(
            "tool skill failed to read {}: {error}",
            base.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            RuntimeError::Provider(format!("tool skill failed to inspect skill dir: {error}"))
        })?;
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let manifest = entry.path().join(SKILL_MANIFEST);
        if manifest.exists() && seen.insert(manifest.clone()) {
            manifests.push(manifest);
        }
    }
    manifests.sort();
    Ok(())
}

fn find_skill_manifest(root_dir: &Path, skill_name: &str) -> Result<Option<PathBuf>, RuntimeError> {
    let path = PathBuf::from(skill_name);
    if path.exists() {
        return Ok(Some(manifest_for_path(&path)?));
    }
    for base in skill_search_roots(root_dir) {
        for candidate in [
            base.join(skill_name).join(SKILL_MANIFEST),
            base.join("vendor").join(skill_name).join(SKILL_MANIFEST),
        ] {
            if candidate.exists() {
                return Ok(Some(candidate));
            }
        }
    }
    Ok(None)
}

fn manifest_for_path(path: &Path) -> Result<PathBuf, RuntimeError> {
    if path.is_dir() {
        let manifest = path.join(SKILL_MANIFEST);
        if manifest.exists() {
            return Ok(manifest);
        }
    } else if path.file_name().and_then(|name| name.to_str()) == Some(SKILL_MANIFEST) {
        return Ok(path.to_path_buf());
    }
    Err(RuntimeError::Provider(format!(
        "tool skill path must be a skill directory or {SKILL_MANIFEST}: {}",
        path.display()
    )))
}

fn skill_search_roots(root_dir: &Path) -> Vec<PathBuf> {
    if root_dir.file_name().and_then(|name| name.to_str()) == Some("skills") {
        vec![root_dir.to_path_buf()]
    } else {
        vec![root_dir.join("skills"), root_dir.to_path_buf()]
    }
}

fn read_manifest(path: &Path) -> Result<SkillManifest, RuntimeError> {
    let content = fs::read_to_string(path).map_err(|error| {
        RuntimeError::Provider(format!(
            "tool skill failed to read manifest {}: {error}",
            path.display()
        ))
    })?;
    serde_yaml::from_str(&content).map_err(|error| {
        RuntimeError::Provider(format!(
            "tool skill failed to parse manifest {}: {error}",
            path.display()
        ))
    })
}

fn instruction_paths(skill_dir: &Path, manifest: &SkillManifest) -> Vec<PathBuf> {
    let manifest_files = manifest
        .instructions
        .files
        .iter()
        .map(|path| skill_dir.join(path))
        .filter(|path| path.exists())
        .collect::<Vec<_>>();
    if !manifest_files.is_empty() {
        return manifest_files;
    }
    ["SKILL.md", "README.md"]
        .iter()
        .map(|name| skill_dir.join(name))
        .filter(|path| path.exists())
        .collect()
}

fn skill_description(skill_dir: &Path, manifest: &SkillManifest) -> Option<String> {
    for path in instruction_paths(skill_dir, manifest) {
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        if let Some(description) = frontmatter_description(&content) {
            return Some(description);
        }
    }
    manifest.description.clone()
}

fn frontmatter_description(content: &str) -> Option<String> {
    let mut lines = content.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    for line in lines {
        let line = line.trim();
        if line == "---" {
            break;
        }
        let Some(value) = line.strip_prefix("description:") else {
            continue;
        };
        let value = value.trim().trim_matches('"').trim_matches('\'');
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

fn read_audit_risk(manifest_path: &Path) -> Option<String> {
    let audit_path = manifest_path.parent()?.join("audit.json");
    let audit = fs::read_to_string(audit_path).ok()?;
    let audit: Value = serde_json::from_str(&audit).ok()?;
    audit
        .get("risk")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
