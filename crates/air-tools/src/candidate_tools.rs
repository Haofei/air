use super::*;

fn validate_test_command_allowlisted(
    name: &str,
    test_command: &str,
    allowed_test_commands: &[String],
) -> Result<(), RuntimeError> {
    if !allowed_test_commands
        .iter()
        .any(|allowed| allowed == test_command)
    {
        return Err(RuntimeError::Provider(format!(
            "tool {name} candidate test_command {test_command} is not allowlisted"
        )));
    }
    Ok(())
}

pub(super) fn call_candidate_validate_tool(
    name: &str,
    input: &Value,
    base_dir: &Path,
    allowed_test_commands: &[String],
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

    let test_command = required_input_string(name, candidate, "test_command")?;
    validate_test_command_allowlisted(name, test_command, allowed_test_commands)?;

    let mut related_files = Vec::new();
    if let Some(files) = candidate.get("related_files").and_then(Value::as_array) {
        for file in files {
            let Some(file) = file.as_str() else {
                return Err(RuntimeError::Provider(format!(
                    "tool {name} candidate.related_files entries must be strings"
                )));
            };
            validate_git_pathspec(name, file)?;
            let absolute =
                canonicalize_tool_path(name, "candidate.related_files", &base_dir.join(file))?;
            if !absolute.starts_with(&base_dir) || !absolute.is_file() {
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

    Ok(json!({
        "valid": true,
        "target_path": target_path,
        "related_files": related_files,
        "test_command": test_command,
        "artifacts": [{
            "id": format!("candidate-validation:{target_path}:{test_command}"),
            "kind": "candidate_validation",
            "title": "refactor candidate validation",
            "uri": format!("air://candidate/{target_path}"),
            "content": format!("valid: true\ntarget_path: {target_path}\ntest_command: {test_command}"),
            "metadata": {
                "provider": "candidate_validate",
                "valid": true,
                "target_path": target_path,
                "test_command": test_command
            }
        }]
    }))
}
