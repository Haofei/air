use super::*;

fn parse_input_files(tool_name: &str, input: &Value) -> Result<Vec<String>, RuntimeError> {
    let Some(raw_files) = input.get("files") else {
        return Ok(Vec::new());
    };
    let raw_files = raw_files.as_array().ok_or_else(|| {
        RuntimeError::Provider(format!("tool {tool_name} input.files must be an array"))
    })?;
    let mut file_paths = Vec::with_capacity(raw_files.len());
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
        file_paths.push(path.to_string());
    }
    Ok(file_paths)
}

pub(super) fn git_diff_paths(
    tool_name: &str,
    input: &Value,
) -> Result<(Vec<String>, bool), RuntimeError> {
    let mut paths = repo_tool_paths(tool_name, input)?;
    let mut has_filter =
        input.get("path").is_some() || input.get("paths").is_some() || input.get("files").is_some();
    let file_paths = parse_input_files(tool_name, input)?;
    if !file_paths.is_empty() {
        has_filter = true;
        paths.extend(file_paths);
    }
    paths.sort();
    paths.dedup();
    Ok((paths, has_filter))
}

pub(crate) fn git_status_label(code: &str) -> &'static str {
    match code {
        "??" => "untracked",
        "!!" => "ignored",
        " M" | "M " | "MM" => "modified",
        " A" | "A " | "AM" => "added",
        " D" | "D " => "deleted",
        "R " | " R" => "renamed",
        "C " | " C" => "copied",
        "UU" | "AA" | "DD" | "AU" | "UA" | "DU" | "UD" => "unmerged",
        _ => "changed",
    }
}
