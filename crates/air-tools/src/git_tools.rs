use super::*;

pub(super) fn git_diff_paths(
    tool_name: &str,
    input: &Value,
) -> Result<(Vec<String>, bool), RuntimeError> {
    let mut paths = repo_tool_paths(tool_name, input)?;
    let mut has_filter = input.get("path").is_some() || input.get("paths").is_some();
    if let Some(raw_files) = input.get("files") {
        has_filter = true;
        let raw_files = raw_files.as_array().ok_or_else(|| {
            RuntimeError::Provider(format!("tool {tool_name} input.files must be an array"))
        })?;
        for (index, file) in raw_files.iter().enumerate() {
            let path = if let Some(path) = file.as_str() {
                path
            } else {
                file.as_object()
                    .and_then(|object| object.get("path"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        RuntimeError::Provider(format!(
                            "tool {tool_name} input.files[{index}] must be a string or object with path"
                        ))
                    })?
            };
            validate_git_pathspec(tool_name, path)?;
            paths.push(path.to_string());
        }
    }
    paths.sort();
    paths.dedup();
    Ok((paths, has_filter))
}
