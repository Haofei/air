use super::*;

pub(crate) fn call_diagnostic_context_tool(
    name: &str,
    input: &Value,
    repo_dir: &Path,
    max_diagnostics: usize,
    context_lines: usize,
    max_bytes: usize,
) -> Result<Value, RuntimeError> {
    let empty_diagnostics = Vec::new();
    let diagnostics = match input.get("diagnostics") {
        Some(diagnostics) => diagnostics.as_array().ok_or_else(|| {
            RuntimeError::Provider(format!("tool {name} input.diagnostics must be an array"))
        })?,
        None => &empty_diagnostics,
    };
    let effective_max_diagnostics =
        optional_bounded_usize_input(name, input, "max_diagnostics", max_diagnostics)?
            .unwrap_or(max_diagnostics);
    let effective_context_lines =
        optional_bounded_usize_input(name, input, "context_lines", context_lines)?
            .unwrap_or(context_lines);
    let repo = canonicalize_tool_path(name, "repo_dir", repo_dir)?;
    let mut files: BTreeMap<PathBuf, DiagnosticContextFile> = BTreeMap::new();
    let mut considered = Vec::new();
    let mut unreadable = Vec::new();

    for (index, diagnostic) in diagnostics
        .iter()
        .take(effective_max_diagnostics)
        .enumerate()
    {
        let path = diagnostic.get("path").and_then(Value::as_str);
        let line = diagnostic.get("line").and_then(Value::as_u64);
        let Some(path) = path else {
            unreadable.push(json!({
                "diagnostic_index": index,
                "reason": "missing path"
            }));
            continue;
        };
        let line = line
            .and_then(|line| usize::try_from(line).ok())
            .filter(|line| *line > 0)
            .unwrap_or(1);
        let line_defaulted = diagnostic
            .get("line")
            .and_then(Value::as_u64)
            .and_then(|line| usize::try_from(line).ok())
            .is_none_or(|line| line == 0);
        let Some((absolute_path, relative_path)) = resolve_diagnostic_context_path(&repo, path)
        else {
            unreadable.push(json!({
                "diagnostic_index": index,
                "path": path,
                "reason": "path is outside repo_dir or cannot be read"
            }));
            continue;
        };
        let body = match fs::read(&absolute_path) {
            Ok(body) => body,
            Err(error) => {
                unreadable.push(json!({
                    "diagnostic_index": index,
                    "path": relative_path,
                    "reason": format!("read failed: {error}")
                }));
                continue;
            }
        };
        if is_likely_binary(&body) {
            unreadable.push(json!({
                "diagnostic_index": index,
                "path": relative_path,
                "reason": "file appears to be binary"
            }));
            continue;
        }
        let content = match std::str::from_utf8(&body) {
            Ok(content) => content.to_string(),
            Err(_) => {
                unreadable.push(json!({
                    "diagnostic_index": index,
                    "path": relative_path,
                    "reason": "file is not valid UTF-8"
                }));
                continue;
            }
        };
        let total_lines = content.lines().count().max(1);
        if line > total_lines {
            unreadable.push(json!({
                "diagnostic_index": index,
                "path": relative_path,
                "line": line,
                "reason": "line is outside file"
            }));
            continue;
        }
        considered.push(diagnostic.clone());
        let file = files
            .entry(absolute_path)
            .or_insert_with(|| DiagnosticContextFile {
                relative_path,
                content,
                total_lines,
                lines: Vec::new(),
                diagnostic_indexes: Vec::new(),
                path_only_diagnostic_indexes: Vec::new(),
            });
        file.lines.push(line);
        file.diagnostic_indexes.push(index);
        if line_defaulted {
            file.path_only_diagnostic_indexes.push(index);
        }
    }

    let mut snippets = Vec::new();
    let mut rendered = String::new();
    let mut truncated = diagnostics.len() > effective_max_diagnostics;
    for file in files.values() {
        for (start_line, end_line) in
            merge_line_ranges(&file.lines, file.total_lines, effective_context_lines)
        {
            let content = numbered_line_range(&file.content, start_line, end_line);
            let remaining = max_bytes.saturating_sub(rendered.len());
            if remaining == 0 {
                truncated = true;
                break;
            }
            let (limited_content, content_truncated, _source_bytes) =
                bytes_to_limited_text(content.as_bytes(), remaining);
            let diagnostic_indexes = file
                .lines
                .iter()
                .zip(file.diagnostic_indexes.iter())
                .filter_map(|(line, index)| {
                    (*line >= start_line && *line <= end_line).then_some(*index)
                })
                .collect::<Vec<_>>();
            let path_only_diagnostic_indexes = diagnostic_indexes
                .iter()
                .copied()
                .filter(|index| file.path_only_diagnostic_indexes.contains(index))
                .collect::<Vec<_>>();
            truncated |= content_truncated;
            if !rendered.is_empty() {
                rendered.push_str("\n\n");
            }
            rendered.push_str(&format!(
                "== {}:{}-{} ==\n{}",
                file.relative_path, start_line, end_line, limited_content
            ));
            snippets.push(json!({
                "path": file.relative_path,
                "start_line": start_line,
                "end_line": end_line,
                "content": limited_content,
                "diagnostic_indexes": diagnostic_indexes,
                "path_only_diagnostic_indexes": path_only_diagnostic_indexes,
                "truncated": content_truncated
            }));
            if content_truncated {
                break;
            }
        }
        if rendered.len() >= max_bytes {
            truncated = true;
            break;
        }
    }
    let bytes = rendered.len();

    Ok(json!({
        "repo": repo.display().to_string(),
        "diagnostics": considered,
        "snippets": snippets,
        "unreadable": unreadable,
        "diagnostic_count": diagnostics.len(),
        "max_diagnostics": effective_max_diagnostics,
        "context_lines": effective_context_lines,
        "bytes": bytes,
        "truncated": truncated,
        "artifacts": [{
            "id": format!("diagnostic-context:{}", repo.display()),
            "kind": "diagnostic_context",
            "title": "diagnostic context",
            "uri": repo.display().to_string(),
            "content": rendered,
            "metadata": {
                "provider": "diagnostic_context",
                "repo": repo.display().to_string(),
                "diagnostic_count": diagnostics.len(),
                "snippet_count": snippets.len(),
                "unreadable_count": unreadable.len(),
                "max_diagnostics": effective_max_diagnostics,
                "context_lines": effective_context_lines,
                "bytes": bytes,
                "truncated": truncated
            }
        }]
    }))
}

fn resolve_diagnostic_context_path(
    repo: &Path,
    diagnostic_path: &str,
) -> Option<(PathBuf, String)> {
    let raw_path = Path::new(diagnostic_path);
    if !raw_path.is_absolute()
        && validate_git_pathspec("diagnostic.context", diagnostic_path).is_err()
    {
        return None;
    }
    let candidate = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        repo.join(raw_path)
    };
    let absolute = candidate.canonicalize().ok()?;
    if !absolute.starts_with(repo) {
        return None;
    }
    let relative = absolute
        .strip_prefix(repo)
        .ok()?
        .to_string_lossy()
        .to_string();
    Some((absolute, relative))
}

struct DiagnosticContextFile {
    relative_path: String,
    content: String,
    total_lines: usize,
    lines: Vec<usize>,
    diagnostic_indexes: Vec<usize>,
    path_only_diagnostic_indexes: Vec<usize>,
}
