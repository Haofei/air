use super::*;

pub(super) fn call_candidate_validate_tool(
    name: &str,
    input: &Value,
    base_dir: &Path,
) -> Result<Value, RuntimeError> {
    let candidate = input.get("candidate").unwrap_or(input);
    let target_path = required_input_string(name, candidate, "target_path")?;
    validate_git_pathspec(name, target_path)?;
    let base_dir = canonicalize_tool_path(name, "base_dir", base_dir)?;
    let target_absolute =
        canonicalize_tool_path(name, "candidate.target_path", &base_dir.join(target_path))?;
    if !target_absolute.starts_with(&base_dir) || !target_absolute.is_file() {
        return Err(RuntimeError::Provider(format!(
            "tool {name} candidate target_path {target_path} is not a readable file under {}",
            base_dir.display()
        )));
    }

    let related_files = validate_related_files(name, candidate, &base_dir, target_path)?;

    Ok(json!({
        "valid": true,
        "target_path": target_path,
        "related_files": related_files,
        "artifacts": [{
            "id": format!("candidate-validation:{target_path}"),
            "kind": "candidate_validation",
            "title": "edit candidate validation",
            "uri": format!("air://candidate/{target_path}"),
            "content": format!("valid: true\ntarget_path: {target_path}"),
            "metadata": {
                "provider": "candidate_validate",
                "valid": true,
                "target_path": target_path
            }
        }]
    }))
}

fn validate_related_files(
    name: &str,
    candidate_input: &Value,
    base_dir: &Path,
    target_path: &str,
) -> Result<Vec<String>, RuntimeError> {
    let mut related_files = Vec::new();
    if let Some(files) = candidate_input
        .get("related_files")
        .and_then(Value::as_array)
    {
        for file in files {
            let Some(file) = file.as_str() else {
                return Err(RuntimeError::Provider(format!(
                    "tool {name} candidate.related_files entries must be strings"
                )));
            };
            validate_git_pathspec(name, file)?;
            let absolute =
                canonicalize_tool_path(name, "candidate.related_files", &base_dir.join(file))?;
            if !absolute.starts_with(base_dir) || !absolute.is_file() {
                return Err(RuntimeError::Provider(format!(
                    "tool {name} candidate related file {file} is not a readable file under {}",
                    base_dir.display()
                )));
            }
            if file != target_path {
                related_files.push(file.to_string());
            }
        }
    }
    Ok(related_files)
}
