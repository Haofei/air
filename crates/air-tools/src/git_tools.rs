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

fn has_explicit_file_filter(input: &Value) -> bool {
    input.get("files").is_some()
}

pub(super) fn git_diff_paths(
    tool_name: &str,
    input: &Value,
) -> Result<(Vec<String>, bool), RuntimeError> {
    let mut paths = repo_tool_paths(tool_name, input)?;
    let explicit_file_filter = has_explicit_file_filter(input);
    let mut has_filter = explicit_file_filter || !paths.is_empty();
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

pub(super) fn git_diff_changed_files(diff: &str) -> Vec<String> {
    let mut files = BTreeSet::new();
    let mut pending_old_path = None::<String>;
    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("--- a/") {
            pending_old_path = Some(clean_git_diff_path(path));
            continue;
        }
        if line == "+++ /dev/null" {
            if let Some(path) = pending_old_path.take() {
                insert_clean_diff_path(&mut files, &path);
            }
            continue;
        }
        if let Some(path) = line.strip_prefix("+++ b/") {
            insert_clean_diff_path(&mut files, path);
            pending_old_path = None;
            continue;
        }
        if let Some(path) = git_diff_header_new_path(line) {
            insert_clean_diff_path(&mut files, &path);
        }
    }
    files.into_iter().collect()
}

fn strip_diff_header_new_path(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("diff --git ")?;
    rest.rsplit_once(" b/")
        .map(|(_, path)| path)
        .or_else(|| rest.strip_prefix("b/"))
}

fn git_diff_header_new_path(line: &str) -> Option<String> {
    Some(clean_git_diff_path(strip_diff_header_new_path(line)?))
}

fn insert_clean_diff_path(files: &mut BTreeSet<String>, path: &str) {
    files.insert(clean_git_diff_path(path));
}

fn clean_git_diff_path(path: &str) -> String {
    path.trim().trim_matches('"').replace('\\', "/")
}

pub(super) fn git_diff_preexisting_changed_files(
    baseline: Option<&BTreeMap<String, Option<Vec<u8>>>>,
    paths: &[String],
    has_filter: bool,
    changed_files: &[String],
) -> Vec<Value> {
    let Some(baseline) = baseline else {
        return Vec::new();
    };
    let changed_files = changed_files.iter().cloned().collect::<BTreeSet<_>>();
    let selected = if has_filter {
        paths.iter().cloned().collect::<BTreeSet<_>>()
    } else {
        baseline.keys().cloned().collect::<BTreeSet<_>>()
    };
    selected
        .into_iter()
        .filter(|path| baseline.contains_key(path) && changed_files.contains(path))
        .map(|path| json!({ "path": path }))
        .collect()
}
