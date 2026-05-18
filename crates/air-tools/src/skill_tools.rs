use air_runtime::RuntimeError;
use air_skill_runtime as skill_runtime;
use air_tools_core::text::bytes_to_limited_text;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

pub(crate) fn call_skill_tool(
    name: &str,
    input: &Value,
    root_dir: &Path,
    max_bytes: usize,
    enabled: bool,
) -> Result<Value, RuntimeError> {
    if !enabled {
        return disabled_skill_tool(name, input);
    }
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

fn disabled_skill_tool(name: &str, input: &Value) -> Result<Value, RuntimeError> {
    if let Some(skill_name) = input
        .get("name")
        .or_else(|| input.get("skill"))
        .or_else(|| input.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Err(RuntimeError::Provider(format!(
            "tool {name} is disabled and cannot load AIR skill `{skill_name}`"
        )));
    }
    if let Some(task) = input
        .get("route_task")
        .or_else(|| input.get("task"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(json!({
            "kind": "skill_route",
            "task": task,
            "disabled": true,
            "executor": {
                "id": "code-agent",
                "reason": "skill routing disabled for this run"
            },
            "instructions": [],
            "skills": [],
            "rejected": [],
            "candidates": [],
        }));
    }
    Ok(json!({
        "kind": "skill_list",
        "disabled": true,
        "skills": [],
    }))
}

fn route_skills(root_dir: &Path, task: &str, top_k: usize) -> Result<Value, RuntimeError> {
    let route = skill_runtime::route_skills_for_task(root_dir, task, top_k)
        .map_err(|error| RuntimeError::Provider(format!("tool skill route: {error}")))?;
    Ok(json!({
        "kind": "skill_route",
        "task": task,
        "executor": route.executor,
        "instructions": route.instructions,
        "skills": route.selected,
        "rejected": route.rejected,
        "candidates": route.candidates,
    }))
}

fn list_skills(root_dir: &Path) -> Result<Value, RuntimeError> {
    let skills = skill_runtime::discover_skills(root_dir)
        .map_err(|error| RuntimeError::Provider(format!("tool skill list: {error}")))?
        .into_iter()
        .map(|skill| {
            let audit = skill_runtime::audit_resolved_skill(&skill).ok();
            json!({
                "id": skill.manifest.id,
                "version": skill.manifest.version,
                "description": skill_runtime::skill_description(&skill),
                "manifest": display_path(&skill.manifest_path),
                "audit_risk": audit.as_ref().map(|audit| audit.risk.clone()),
                "allowed_to_run": audit.as_ref().map(|audit| audit.allowed_to_run),
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
    let manifest_path = skill_runtime::find_skill_manifest(root_dir, skill_name)
        .map_err(|error| RuntimeError::Provider(format!("tool {tool_name}: {error}")))?
        .ok_or_else(|| {
            RuntimeError::Provider(format!(
                "tool {tool_name} could not find AIR skill `{skill_name}`"
            ))
        })?;
    let skill = skill_runtime::read_skill_manifest(&manifest_path)
        .map_err(|error| RuntimeError::Provider(format!("tool {tool_name}: {error}")))?;
    let audit = skill_runtime::audit_resolved_skill(&skill)
        .map_err(|error| RuntimeError::Provider(format!("tool {tool_name} audit: {error}")))?;
    if !audit.allowed_to_run {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} refused to load AIR skill `{skill_name}` because audit risk is {}",
            audit.risk
        )));
    }
    let instruction_paths = skill_runtime::instruction_paths(&skill);
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
            "id": format!("skill:{}:{}", skill.manifest.id, path.file_name().and_then(|name| name.to_str()).unwrap_or("instruction")),
            "kind": "skill_instruction",
            "title": format!("{} instruction", skill.manifest.id),
            "uri": path_ref,
            "content": "",
            "metadata": {
                "provider": "skill",
                "skill": skill.manifest.id,
            }
        }));
        if remaining == 0 {
            break;
        }
    }
    Ok(json!({
        "kind": "skill_instruction",
        "skill": {
            "id": skill.manifest.id,
            "version": skill.manifest.version,
            "description": skill_runtime::skill_description(&skill),
            "manifest": display_path(&manifest_path),
            "audit_risk": audit.risk,
            "allowed_to_run": audit.allowed_to_run,
        },
        "instructions": instructions,
        "artifacts": artifacts,
    }))
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
