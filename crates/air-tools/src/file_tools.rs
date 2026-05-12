use super::*;
use std::io::Write;

pub(super) fn call_file_read_tool(
    name: &str,
    input: &Value,
    base_dir: &Path,
    max_bytes: usize,
) -> Result<Value, RuntimeError> {
    let input_path = required_input_string(name, input, "path")?;
    let base = canonicalize_tool_path(name, "base_dir", base_dir)?;
    let candidate = if Path::new(input_path).is_absolute() {
        PathBuf::from(input_path)
    } else {
        base.join(input_path)
    };
    let path = canonicalize_tool_path(name, "input.path", &candidate)?;
    if !path.starts_with(&base) {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.path is outside configured base_dir"
        )));
    }
    let body = fs::read(&path)
        .map_err(|error| RuntimeError::Provider(format!("tool {name} read file: {error}")))?;
    if is_likely_binary(&body) {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.path appears to be binary; file.read only supports UTF-8 text"
        )));
    }
    let full_content = std::str::from_utf8(&body)
        .map_err(|_| {
            RuntimeError::Provider(format!(
                "tool {name} input.path is not valid UTF-8; file.read only supports UTF-8 text"
            ))
        })?
        .to_string();
    let total_lines = full_content.lines().count();
    let lines_range = optional_line_range_input(name, input, "lines")?;
    if lines_range.is_some()
        && (input.get("start_line").is_some() || input.get("end_line").is_some())
    {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.lines cannot be combined with input.start_line or input.end_line"
        )));
    }
    let explicit_start_line = optional_positive_usize_input(name, input, "start_line")?;
    let explicit_end_line = optional_positive_usize_input(name, input, "end_line")?;
    let (start_line, end_line) = if let Some((start, end)) = lines_range {
        (Some(start), Some(end))
    } else {
        (explicit_start_line, explicit_end_line)
    };
    let contains = optional_string_input(name, input, "contains")?;
    let context_lines = optional_positive_usize_input(name, input, "context_lines")?.unwrap_or(0);
    let occurrence = optional_positive_usize_input(name, input, "occurrence")?.unwrap_or(1);
    let explicit_line_numbers = input.get("line_numbers").is_some();
    let line_numbers = optional_bool_input(name, input, "line_numbers")?
        .unwrap_or_else(|| start_line.is_some() || end_line.is_some());
    let effective_max_bytes =
        optional_bounded_usize_input(name, input, "max_bytes", max_bytes)?.unwrap_or(max_bytes);
    if let (Some(start), Some(end)) = (start_line, end_line) {
        if start > end {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.start_line must be less than or equal to input.end_line"
            )));
        }
    }
    let (effective_start_line, effective_end_line, match_line) = if let Some(needle) = contains {
        if start_line.is_some() || end_line.is_some() {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.contains cannot be combined with input.start_line or input.end_line"
            )));
        }
        if needle.is_empty() {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.contains must not be empty"
            )));
        }
        let Some(index) = full_content
            .lines()
            .enumerate()
            .filter_map(|(index, line)| line.contains(needle).then_some(index))
            .nth(occurrence - 1)
        else {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.contains occurrence={occurrence} was not found"
            )));
        };
        let line = index + 1;
        let start = line.saturating_sub(context_lines).max(1);
        let end = (line + context_lines).min(total_lines);
        (Some(start), Some(end), Some(line))
    } else {
        if input.get("occurrence").is_some() {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.occurrence requires input.contains"
            )));
        }
        if input.get("context_lines").is_some() {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.context_lines requires input.contains"
            )));
        }
        (start_line, end_line, None)
    };
    let selected = select_line_range(&full_content, effective_start_line, effective_end_line);
    let (selected_content, truncated, bytes) =
        bytes_to_limited_text(selected.as_bytes(), effective_max_bytes);
    let full_output_path =
        maybe_save_full_output(name, &base, "file-read", selected.as_bytes(), truncated)?;
    let content = if line_numbers {
        numbered_content(&selected_content, effective_start_line.unwrap_or(1))
    } else {
        selected_content
    };
    let unscoped_read = effective_start_line.is_none()
        && effective_end_line.is_none()
        && match_line.is_none()
        && body.len() > effective_max_bytes;
    let truncation_hint = file_read_truncation_hint(
        truncated,
        unscoped_read,
        body.len(),
        full_output_path.as_deref(),
    );
    let artifact = json!({
        "id": format!("file:{}", path.display()),
        "kind": "file_span",
        "title": path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_else(|| path.to_str().unwrap_or("file")),
        "uri": path.display().to_string(),
        "metadata": {
            "provider": "file_read",
            "path": path.display().to_string(),
            "bytes": bytes,
            "source_bytes": body.len(),
            "start_line": effective_start_line,
            "end_line": effective_end_line,
            "match_line": match_line,
            "contains": contains,
            "context_lines": context_lines,
            "occurrence": occurrence,
            "total_lines": total_lines,
            "truncated": truncated,
            "full_output_path": full_output_path,
            "unscoped_read": unscoped_read,
            "line_numbers": line_numbers,
            "line_numbers_defaulted": !explicit_line_numbers && line_numbers
        }
    });
    Ok(json!({
        "path": path.display().to_string(),
        "content": content,
        "content_format": if line_numbers { "line_numbered" } else { "plain" },
        "bytes": bytes,
        "source_bytes": body.len(),
        "start_line": effective_start_line,
        "end_line": effective_end_line,
        "match_line": match_line,
        "contains": contains,
        "context_lines": context_lines,
        "occurrence": occurrence,
        "total_lines": total_lines,
        "max_bytes": effective_max_bytes,
        "truncated": truncated,
        "full_output_path": full_output_path,
        "truncation_hint": truncation_hint,
        "unscoped_read": unscoped_read,
        "line_numbers": line_numbers,
        "line_numbers_defaulted": !explicit_line_numbers && line_numbers,
        "artifacts": [artifact],
    }))
}

fn file_read_truncation_hint(
    truncated: bool,
    unscoped_read: bool,
    source_bytes: usize,
    full_output_path: Option<&str>,
) -> Option<String> {
    truncated.then(|| {
        let saved = full_output_path
            .map(|path| format!(" Full selected output saved to: {path}."))
            .unwrap_or_default();
        if unscoped_read {
            format!(
                "Output was truncated from {} bytes.{saved} Use file.search, contains+context_lines, or file.read with start_line/end_line to inspect a narrow range instead of repeating an unscoped read.",
                source_bytes,
            )
        } else {
            format!(
                "Output was truncated from the selected range.{saved} Increase max_bytes only if the whole selected range is immediately needed, otherwise use a narrower line range."
            )
        }
    })
}

pub(super) fn call_file_read_many_tool(
    name: &str,
    input: &Value,
    base_dir: &Path,
    max_bytes: usize,
    max_files: usize,
) -> Result<Value, RuntimeError> {
    let entries = file_read_many_entries(name, input)?;
    if entries.is_empty() {
        return Ok(json!({
            "files": [],
            "file_count": 0,
            "bytes": 0,
            "truncated": false,
            "artifacts": []
        }));
    }
    if entries.len() > max_files {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.files must contain at most {max_files} files"
        )));
    }
    let effective_max_bytes =
        optional_bounded_usize_input(name, input, "max_bytes_per_file", max_bytes)?
            .unwrap_or(max_bytes);

    let mut files = Vec::with_capacity(entries.len());
    let mut artifacts = Vec::new();
    let mut total_bytes = 0usize;
    let mut any_truncated = false;
    for entry in entries {
        let output = call_file_read_tool(name, &entry, base_dir, effective_max_bytes)?;
        total_bytes += output.get("bytes").and_then(Value::as_u64).unwrap_or(0) as usize;
        any_truncated |= output
            .get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if let Some(entry_artifacts) = output.get("artifacts").and_then(Value::as_array) {
            artifacts.extend(entry_artifacts.iter().cloned());
        }
        files.push(output);
    }

    let file_count = files.len();
    Ok(json!({
        "files": files,
        "file_count": file_count,
        "bytes": total_bytes,
        "max_bytes_per_file": effective_max_bytes,
        "truncated": any_truncated,
        "artifacts": artifacts
    }))
}

fn file_read_many_entries(name: &str, input: &Value) -> Result<Vec<Value>, RuntimeError> {
    let Some(files_value) = input.get("files").or_else(|| input.get("paths")) else {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.files must be an array"
        )));
    };
    let files = files_value.as_array().ok_or_else(|| {
        RuntimeError::Provider(format!("tool {name} input.files must be an array"))
    })?;
    files
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            if let Some(path) = entry.as_str() {
                Ok(json!({ "path": path }))
            } else if entry.is_object() {
                let path = entry.get("path").and_then(Value::as_str).ok_or_else(|| {
                    RuntimeError::Provider(format!(
                        "tool {name} input.files[{index}].path must be a string"
                    ))
                })?;
                if path.is_empty() {
                    return Err(RuntimeError::Provider(format!(
                        "tool {name} input.files[{index}].path must not be empty"
                    )));
                }
                Ok(entry.clone())
            } else {
                Err(RuntimeError::Provider(format!(
                    "tool {name} input.files[{index}] must be a string path or object"
                )))
            }
        })
        .collect()
}

pub(super) fn call_file_search_tool(
    name: &str,
    input: &Value,
    base_dir: &Path,
    max_bytes: usize,
    max_matches: usize,
    max_context_lines: usize,
    max_line_chars: usize,
) -> Result<Value, RuntimeError> {
    let input_path = required_input_string(name, input, "path")?;
    let (pattern, pattern_source) = file_search_pattern_input(name, input)?;
    if pattern.is_empty() {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.pattern must not be empty"
        )));
    }
    let regex = regex::Regex::new(pattern)
        .map_err(|error| RuntimeError::Provider(format!("tool {name} invalid regex: {error}")))?;
    let base = canonicalize_tool_path(name, "base_dir", base_dir)?;
    let candidate = if Path::new(input_path).is_absolute() {
        PathBuf::from(input_path)
    } else {
        base.join(input_path)
    };
    let path = canonicalize_tool_path(name, "input.path", &candidate)?;
    if !path.starts_with(&base) {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.path is outside configured base_dir"
        )));
    }
    let context_lines = optional_bounded_zero_or_positive_usize_input(
        name,
        input,
        "context_lines",
        max_context_lines,
    )?
    .unwrap_or(0);
    let effective_max_matches =
        optional_bounded_usize_input(name, input, "max_matches", max_matches)?
            .unwrap_or(max_matches);
    let effective_max_line_chars =
        optional_bounded_usize_input(name, input, "max_line_chars", max_line_chars)?
            .unwrap_or(max_line_chars);
    let directory = path.is_dir();
    let mut result = FileSearchResult::default();
    if directory {
        for file in file_search_directory_files(name, &base, &path)? {
            let relative_path = file
                .strip_prefix(&base)
                .unwrap_or(file.as_path())
                .to_string_lossy()
                .replace('\\', "/");
            search_file_path(
                &file,
                Some(&relative_path),
                &regex,
                context_lines,
                effective_max_matches,
                effective_max_line_chars,
                &mut result,
            )?;
        }
    } else {
        search_file_path(
            &path,
            None,
            &regex,
            context_lines,
            effective_max_matches,
            effective_max_line_chars,
            &mut result,
        )?;
    }
    let mut rendered = String::new();
    for item in &result.matches {
        for context in item
            .get("before")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            rendered.push_str(&render_search_line(context));
        }
        rendered.push_str(&render_search_line(item));
        for context in item
            .get("after")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            rendered.push_str(&render_search_line(context));
        }
    }
    let (artifact_content, content_truncated, bytes) =
        bytes_to_limited_text(rendered.as_bytes(), max_bytes);
    let full_output_path = maybe_save_full_output(
        name,
        &base,
        "file-search",
        rendered.as_bytes(),
        content_truncated,
    )?;
    let match_truncated = result.total_match_count > result.matches.len();
    let truncated = match_truncated || content_truncated || result.any_line_truncated;
    let truncation_hint = file_search_truncation_hint(
        content_truncated,
        match_truncated,
        full_output_path.as_deref(),
    );
    let artifact = json!({
        "id": format!("file-search:{}:{}", path.display(), stable_pattern_id(pattern)),
        "kind": "file_search",
        "title": format!(
            "{} matches in {}",
            result.total_match_count,
            path.file_name().and_then(|name| name.to_str()).unwrap_or("file")
        ),
        "uri": path.display().to_string(),
        "content": artifact_content,
        "metadata": {
            "provider": "file_search",
            "path": path.display().to_string(),
            "directory": directory,
            "pattern": pattern,
            "pattern_source": pattern_source,
            "match_count": result.total_match_count,
            "returned_match_count": result.matches.len(),
            "total_lines": result.total_lines,
            "searched_file_count": result.searched_file_count,
            "skipped_file_count": result.skipped_file_count,
            "context_lines": context_lines,
            "max_matches": effective_max_matches,
            "max_line_chars": effective_max_line_chars,
            "line_truncated": result.any_line_truncated,
            "truncated": truncated,
            "full_output_path": full_output_path,
            "truncation_hint": truncation_hint
        }
    });
    Ok(json!({
        "path": path.display().to_string(),
        "directory": directory,
        "pattern": pattern,
        "pattern_source": pattern_source,
        "matches": result.matches,
        "match_count": result.total_match_count,
        "returned_match_count": result.matches.len(),
        "total_lines": result.total_lines,
        "searched_file_count": result.searched_file_count,
        "skipped_file_count": result.skipped_file_count,
        "context_lines": context_lines,
        "max_matches": effective_max_matches,
        "max_line_chars": effective_max_line_chars,
        "line_truncated": result.any_line_truncated,
        "truncated": truncated,
        "full_output_path": full_output_path,
        "truncation_hint": truncation_hint,
        "bytes": bytes,
        "artifacts": [artifact]
    }))
}

fn file_search_truncation_hint(
    content_truncated: bool,
    match_truncated: bool,
    full_output_path: Option<&str>,
) -> Option<String> {
    if !content_truncated && !match_truncated {
        return None;
    }
    let mut parts = Vec::new();
    if content_truncated {
        if let Some(path) = full_output_path {
            parts.push(format!("Full returned search preview saved to: {path}."));
        }
        parts.push("Use file.search with a narrower pattern or file.read with exact line ranges to inspect specific sections.".to_string());
    }
    if match_truncated {
        parts.push("Search matches exceeded max_matches; narrow the pattern/path or increase max_matches only when the broader match set is immediately needed.".to_string());
    }
    Some(parts.join(" "))
}

fn maybe_save_full_output(
    tool_name: &str,
    base_dir: &Path,
    prefix: &str,
    content: &[u8],
    truncated: bool,
) -> Result<Option<String>, RuntimeError> {
    if truncated {
        Ok(Some(save_text_tool_output(
            tool_name, base_dir, prefix, content,
        )?))
    } else {
        Ok(None)
    }
}

fn save_text_tool_output(
    tool_name: &str,
    base_dir: &Path,
    prefix: &str,
    content: &[u8],
) -> Result<String, RuntimeError> {
    let output_dir = base_dir.join(".air").join("tool-output");
    fs::create_dir_all(&output_dir).map_err(|error| {
        RuntimeError::Provider(format!("tool {tool_name} create output dir: {error}"))
    })?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| RuntimeError::Provider(format!("tool {tool_name} timestamp: {error}")))?
        .as_millis();
    let path = output_dir.join(format!("{prefix}-{timestamp}.log"));
    fs::write(&path, content).map_err(|error| {
        RuntimeError::Provider(format!("tool {tool_name} write full output: {error}"))
    })?;
    Ok(path.display().to_string())
}

fn file_search_pattern_input<'a>(
    name: &str,
    input: &'a Value,
) -> Result<(&'a str, &'static str), RuntimeError> {
    if let Some(pattern) = input.get("pattern") {
        return pattern
            .as_str()
            .map(|pattern| (pattern, "pattern"))
            .ok_or_else(|| {
                RuntimeError::Provider(format!("tool {name} input.pattern must be a string"))
            });
    }
    if let Some(query) = input.get("query") {
        return query.as_str().map(|query| (query, "query")).ok_or_else(|| {
            RuntimeError::Provider(format!("tool {name} input.query must be a string"))
        });
    }
    Err(RuntimeError::Provider(format!(
        "tool {name} input.pattern must be a string"
    )))
}

#[derive(Default)]
struct FileSearchResult {
    matches: Vec<Value>,
    total_match_count: usize,
    total_lines: usize,
    searched_file_count: usize,
    skipped_file_count: usize,
    any_line_truncated: bool,
}

fn search_file_path(
    path: &Path,
    relative_path: Option<&str>,
    regex: &regex::Regex,
    context_lines: usize,
    effective_max_matches: usize,
    effective_max_line_chars: usize,
    result: &mut FileSearchResult,
) -> Result<(), RuntimeError> {
    let body = fs::read(path)
        .map_err(|error| RuntimeError::Provider(format!("tool file.search read file: {error}")))?;
    if is_likely_binary(&body) {
        result.skipped_file_count += 1;
        return Ok(());
    }
    let Ok(content) = std::str::from_utf8(&body) else {
        result.skipped_file_count += 1;
        return Ok(());
    };
    result.searched_file_count += 1;
    let lines = content.lines().collect::<Vec<_>>();
    result.total_lines += lines.len();
    let total_lines = lines.len();
    for (index, line) in lines.iter().enumerate() {
        if !regex.is_match(line) {
            continue;
        }
        result.total_match_count += 1;
        if result.matches.len() >= effective_max_matches {
            continue;
        }
        let line_number = index + 1;
        let before_start = line_number.saturating_sub(context_lines).max(1);
        let before = (before_start..line_number)
            .filter_map(|number| {
                line_json_for_path(&lines, number, effective_max_line_chars, relative_path)
            })
            .inspect(|line| {
                result.any_line_truncated |= line
                    .get("line_truncated")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            })
            .collect::<Vec<_>>();
        let after_end = (line_number + context_lines).min(total_lines);
        let after = ((line_number + 1)..=after_end)
            .filter_map(|number| {
                line_json_for_path(&lines, number, effective_max_line_chars, relative_path)
            })
            .inspect(|line| {
                result.any_line_truncated |= line
                    .get("line_truncated")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            })
            .collect::<Vec<_>>();
        let (line, line_truncated) = truncate_line_text(line, effective_max_line_chars);
        result.any_line_truncated |= line_truncated;
        let mut item = json!({
            "line_number": line_number,
            "line": line,
            "line_truncated": line_truncated,
            "before": before,
            "after": after,
        });
        if let Some(path) = relative_path {
            item["path"] = Value::String(path.to_string());
        }
        result.matches.push(item);
    }
    Ok(())
}

fn file_search_directory_files(
    tool_name: &str,
    base: &Path,
    root: &Path,
) -> Result<Vec<PathBuf>, RuntimeError> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(path) = pending.pop() {
        let entries = fs::read_dir(&path).map_err(|error| {
            RuntimeError::Provider(format!("tool {tool_name} read directory: {error}"))
        })?;
        let mut entries = entries.collect::<Result<Vec<_>, _>>().map_err(|error| {
            RuntimeError::Provider(format!("tool {tool_name} read directory entry: {error}"))
        })?;
        entries.sort_by_key(|entry| entry.path());
        for entry in entries.into_iter().rev() {
            let path = entry.path();
            let path = canonicalize_tool_path(tool_name, "input.path", &path)?;
            if !path.starts_with(base) {
                continue;
            }
            if path.is_dir() {
                if should_skip_file_search_dir(&path) {
                    continue;
                }
                pending.push(path);
            } else if path.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn should_skip_file_search_dir(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(".git" | "target" | "node_modules" | "dist" | "build" | "__pycache__")
    )
}

fn line_json_for_path(
    lines: &[&str],
    line_number: usize,
    max_line_chars: usize,
    path: Option<&str>,
) -> Option<Value> {
    lines.get(line_number.checked_sub(1)?).map(|line| {
        let (line, line_truncated) = truncate_line_text(line, max_line_chars);
        let mut value = json!({
            "line_number": line_number,
            "line": line,
            "line_truncated": line_truncated,
        });
        if let Some(path) = path {
            value["path"] = Value::String(path.to_string());
        }
        value
    })
}

fn truncate_line_text(line: &str, max_line_chars: usize) -> (String, bool) {
    if line.chars().count() <= max_line_chars {
        return (line.to_string(), false);
    }
    (line.chars().take(max_line_chars).collect(), true)
}

fn render_search_line(value: &Value) -> String {
    let number = value
        .get("line_number")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let line = value
        .get("line")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Some(path) = value.get("path").and_then(Value::as_str) {
        format!("{path}:{number}: {line}\n")
    } else {
        format!("{number}: {line}\n")
    }
}

fn stable_pattern_id(pattern: &str) -> String {
    pattern
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .chars()
        .take(48)
        .collect::<String>()
}

fn optional_bounded_zero_or_positive_usize_input(
    tool_name: &str,
    input: &Value,
    field: &str,
    configured_max: usize,
) -> Result<Option<usize>, RuntimeError> {
    let Some(value) = input.get(field) else {
        return Ok(None);
    };
    let Some(number) = value.as_u64() else {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.{field} must be a non-negative integer"
        )));
    };
    usize::try_from(number)
        .map(|value| Some(value.min(configured_max)))
        .map_err(|_| RuntimeError::Provider(format!("tool {tool_name} input.{field} is too large")))
}

fn optional_line_range_input(
    tool_name: &str,
    input: &Value,
    field: &str,
) -> Result<Option<(usize, usize)>, RuntimeError> {
    let Some(value) = input.get(field) else {
        return Ok(None);
    };
    match value {
        Value::String(text) => parse_line_range_text(tool_name, field, text).map(Some),
        Value::Array(items) if items.len() == 2 => {
            let start = positive_usize_from_value(tool_name, field, &items[0])?;
            let end = positive_usize_from_value(tool_name, field, &items[1])?;
            validate_line_range(tool_name, field, start, end).map(Some)
        }
        Value::Number(_) => {
            let line = positive_usize_from_value(tool_name, field, value)?;
            Ok(Some((line, line)))
        }
        _ => Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.{field} must be a line number, \"start-end\", or [start, end]"
        ))),
    }
}

fn parse_line_range_text(
    tool_name: &str,
    field: &str,
    text: &str,
) -> Result<(usize, usize), RuntimeError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.{field} must not be empty"
        )));
    }
    if let Some((start, end)) = text.split_once('-').or_else(|| text.split_once(':')) {
        let start = positive_usize_from_str(tool_name, field, start.trim())?;
        let end = positive_usize_from_str(tool_name, field, end.trim())?;
        return validate_line_range(tool_name, field, start, end);
    }
    let line = positive_usize_from_str(tool_name, field, text)?;
    Ok((line, line))
}

fn positive_usize_from_value(
    tool_name: &str,
    field: &str,
    value: &Value,
) -> Result<usize, RuntimeError> {
    let Some(number) = value.as_u64() else {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.{field} must contain positive integers"
        )));
    };
    usize::try_from(number)
        .ok()
        .filter(|number| *number > 0)
        .ok_or_else(|| {
            RuntimeError::Provider(format!("tool {tool_name} input.{field} is too large"))
        })
}

fn positive_usize_from_str(
    tool_name: &str,
    field: &str,
    value: &str,
) -> Result<usize, RuntimeError> {
    value
        .parse::<usize>()
        .ok()
        .filter(|number| *number > 0)
        .ok_or_else(|| {
            RuntimeError::Provider(format!(
                "tool {tool_name} input.{field} must contain positive integers"
            ))
        })
}

fn validate_line_range(
    tool_name: &str,
    field: &str,
    start: usize,
    end: usize,
) -> Result<(usize, usize), RuntimeError> {
    if start > end {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.{field} start must be less than or equal to end"
        )));
    }
    Ok((start, end))
}

fn numbered_content(content: &str, first_line: usize) -> String {
    content
        .lines()
        .enumerate()
        .map(|(index, line)| format!("{:05}| {}", first_line + index, line))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn is_likely_binary(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let sample = &bytes[..bytes.len().min(4096)];
    if sample.contains(&0) {
        return true;
    }
    let non_printable = sample
        .iter()
        .filter(|byte| **byte < 9 || (**byte > 13 && **byte < 32))
        .count();
    non_printable * 10 > sample.len() * 3
}

pub(super) fn file_modified_time(
    name: &str,
    label: &str,
    path: &Path,
) -> Result<SystemTime, RuntimeError> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|error| {
            RuntimeError::Provider(format!(
                "tool {name} read modification time for {label}: {error}"
            ))
        })
}

fn require_fresh_read(
    name: &str,
    label: &str,
    action: &str,
    path: &Path,
    read_snapshots: &BTreeMap<PathBuf, SystemTime>,
) -> Result<(), RuntimeError> {
    let Some(read_modified) = read_snapshots.get(path) else {
        return Err(RuntimeError::Provider(format!(
            "tool {name} {label} must be read before {action} when require_read is true"
        )));
    };
    let current_modified = file_modified_time(name, label, path)?;
    if current_modified > *read_modified {
        return Err(RuntimeError::Provider(format!(
            "tool {name} {label} was modified after it was last read; read it again before {action}"
        )));
    }
    Ok(())
}

macro_rules! define_edit_match_strategies {
    ($($variant:ident => $name:literal),* $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum EditMatchStrategy {
            Auto,
            $($variant),*
        }

        impl EditMatchStrategy {
            fn from_input(tool_name: &str, input: &Value) -> Result<Self, RuntimeError> {
                Self::from_labeled_input(tool_name, input, "input")
            }

            fn from_labeled_input(
                tool_name: &str,
                input: &Value,
                label: &str,
            ) -> Result<Self, RuntimeError> {
                let Some(value) = optional_labeled_string_input(tool_name, input, label, "match_strategy")?
                else {
                    return Ok(Self::Exact);
                };
                match value {
                    "auto" => Ok(Self::Auto),
                    $($name => Ok(Self::$variant),)*
                    _ => {
                        let valid = ["auto", $($name),*].join(", ");
                        Err(RuntimeError::Provider(format!(
                            "tool {tool_name} {label}.match_strategy must be one of {valid}"
                        )))
                    }
                }
            }

            fn as_str(self) -> &'static str {
                match self {
                    Self::Auto => "auto",
                    $(Self::$variant => $name,)*
                }
            }

            fn concrete_strategies() -> &'static [EditMatchStrategy] {
                &[$(EditMatchStrategy::$variant),*]
            }
        }
    }
}

define_edit_match_strategies! {
    Exact => "exact",
    LineTrimmed => "line_trimmed",
    BlockAnchor => "block_anchor",
    WhitespaceNormalized => "whitespace_normalized",
    IndentationFlexible => "indentation_flexible",
    EscapeNormalized => "escape_normalized",
    TrimmedBoundary => "trimmed_boundary",
    ContextAware => "context_aware",
    MultiOccurrence => "multi_occurrence",
}

const MAX_FILE_EDIT_OPERATIONS: usize = 20;

#[derive(Debug, Clone)]
struct FileEditOperation<'a> {
    label: String,
    old_string: &'a str,
    new_string: &'a str,
    replace_all: bool,
    match_strategy: EditMatchStrategy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EditMatch {
    start: usize,
    end: usize,
}

fn find_edit_matches(
    content: &str,
    old_string: &str,
    strategy: EditMatchStrategy,
) -> (EditMatchStrategy, Vec<EditMatch>) {
    if strategy == EditMatchStrategy::Auto {
        let mut first_non_empty = None;
        for &candidate in EditMatchStrategy::concrete_strategies() {
            let matches = find_edit_matches_with_strategy(content, old_string, candidate);
            if matches.is_empty() {
                continue;
            }
            if matches.len() == 1 {
                return (candidate, matches);
            }
            if first_non_empty.is_none() {
                first_non_empty = Some((candidate, matches));
            }
        }
        return first_non_empty.unwrap_or((EditMatchStrategy::Exact, Vec::new()));
    }

    (
        strategy,
        find_edit_matches_with_strategy(content, old_string, strategy),
    )
}

fn find_edit_matches_with_strategy(
    content: &str,
    old_string: &str,
    strategy: EditMatchStrategy,
) -> Vec<EditMatch> {
    match strategy {
        EditMatchStrategy::Auto => unreachable!("auto expands before concrete matching"),
        EditMatchStrategy::Exact | EditMatchStrategy::MultiOccurrence => content
            .match_indices(old_string)
            .map(|(start, value)| EditMatch {
                start,
                end: start + value.len(),
            })
            .collect(),
        EditMatchStrategy::LineTrimmed => {
            find_line_based_edit_matches(content, old_string, |line| line.trim().to_string())
        }
        EditMatchStrategy::BlockAnchor => find_block_anchor_edit_matches(content, old_string),
        EditMatchStrategy::WhitespaceNormalized => {
            find_whitespace_normalized_edit_matches(content, old_string)
        }
        EditMatchStrategy::IndentationFlexible => {
            find_line_based_edit_matches(content, old_string, strip_common_indentation)
        }
        EditMatchStrategy::EscapeNormalized => {
            find_escape_normalized_edit_matches(content, old_string)
        }
        EditMatchStrategy::TrimmedBoundary => {
            find_trimmed_boundary_edit_matches(content, old_string)
        }
        EditMatchStrategy::ContextAware => find_context_aware_edit_matches(content, old_string),
    }
}

fn selected_edit_matches(matches: &[EditMatch], replace_all: bool) -> Vec<EditMatch> {
    if replace_all {
        matches.to_vec()
    } else {
        vec![matches[0]]
    }
}

fn apply_selected_edit_matches(content: &str, selected: &[EditMatch], new_string: &str) -> String {
    let mut updated = content.to_string();
    for edit_match in selected.iter().rev() {
        updated.replace_range(edit_match.start..edit_match.end, new_string);
    }
    updated
}

fn edit_unified_diff(
    path: &str,
    content: &str,
    selected: &[EditMatch],
    new_string: &str,
) -> String {
    let mut diff = format!("--- a/{path}\n+++ b/{path}\n");
    for edit_match in selected {
        let old_segment = &content[edit_match.start..edit_match.end];
        let old_start_line = line_number_at(content, edit_match.start);
        let old_line_count = diff_line_count(old_segment);
        let new_line_count = diff_line_count(new_string);
        diff.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            old_start_line, old_line_count, old_start_line, new_line_count
        ));
        for line in diff_lines(old_segment) {
            diff.push('-');
            diff.push_str(line);
            diff.push('\n');
        }
        for line in diff_lines(new_string) {
            diff.push('+');
            diff.push_str(line);
            diff.push('\n');
        }
    }
    diff
}

fn line_number_at(content: &str, byte_index: usize) -> usize {
    content[..byte_index]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

fn diff_line_count(text: &str) -> usize {
    diff_lines(text).len().max(1)
}

fn diff_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        Vec::new()
    } else {
        text.lines().collect()
    }
}

fn find_line_based_edit_matches<F>(content: &str, old_string: &str, normalize: F) -> Vec<EditMatch>
where
    F: Fn(&str) -> String,
{
    let content_lines = split_lines_with_offsets(content);
    let mut search_lines = old_string.split('\n').collect::<Vec<_>>();
    if search_lines.last() == Some(&"") {
        search_lines.pop();
    }
    if search_lines.is_empty() || search_lines.len() > content_lines.len() {
        return Vec::new();
    }
    let normalized_search = search_lines
        .iter()
        .map(|line| normalize(line))
        .collect::<Vec<_>>();
    let mut matches = Vec::new();
    for start_line in 0..=content_lines.len() - search_lines.len() {
        let mut matched = true;
        for (offset, expected) in normalized_search.iter().enumerate() {
            if normalize(content_lines[start_line + offset].text) != *expected {
                matched = false;
                break;
            }
        }
        if matched {
            matches.push(EditMatch {
                start: content_lines[start_line].start,
                end: content_lines[start_line + search_lines.len() - 1].end,
            });
        }
    }
    matches
}

fn find_block_anchor_edit_matches(content: &str, old_string: &str) -> Vec<EditMatch> {
    let content_lines = split_lines_with_offsets(content);
    let search_lines = normalized_search_lines(old_string);
    if search_lines.len() < 3 {
        return Vec::new();
    }
    let first_line = search_lines[0].trim();
    let last_line = search_lines[search_lines.len() - 1].trim();
    if first_line.is_empty() || last_line.is_empty() {
        return Vec::new();
    }

    let mut candidates = Vec::new();
    for start_line in 0..content_lines.len() {
        if content_lines[start_line].text.trim() != first_line {
            continue;
        }
        for (end_line, line) in content_lines.iter().enumerate().skip(start_line + 2) {
            if line.text.trim() == last_line {
                candidates.push((start_line, end_line));
                break;
            }
        }
    }
    if candidates.is_empty() {
        return Vec::new();
    }
    if candidates.len() == 1 {
        let (start_line, end_line) = candidates[0];
        return vec![line_span_match(&content_lines, start_line, end_line)];
    }

    let mut best = None;
    let mut best_similarity = -1.0_f64;
    for (start_line, end_line) in candidates {
        let similarity =
            anchored_block_similarity(&content_lines, &search_lines, start_line, end_line);
        if similarity > best_similarity {
            best_similarity = similarity;
            best = Some((start_line, end_line));
        }
    }
    if best_similarity < 0.3 {
        return Vec::new();
    }
    let Some((start_line, end_line)) = best else {
        return Vec::new();
    };
    vec![line_span_match(&content_lines, start_line, end_line)]
}

fn find_whitespace_normalized_edit_matches(content: &str, old_string: &str) -> Vec<EditMatch> {
    let mut matches = Vec::new();
    let normalized_search = normalize_whitespace(old_string);
    if normalized_search.is_empty() {
        return matches;
    }

    let content_lines = split_lines_with_offsets(content);
    if !old_string.contains('\n') {
        for line in &content_lines {
            let normalized_line = normalize_whitespace(line.text);
            if normalized_line == normalized_search {
                matches.push(EditMatch {
                    start: line.start,
                    end: line.end,
                });
                continue;
            }
            if !normalized_line.contains(&normalized_search) {
                continue;
            }
            let words = old_string.split_whitespace().collect::<Vec<_>>();
            if words.is_empty() {
                continue;
            }
            let pattern = words
                .iter()
                .map(|word| regex::escape(word))
                .collect::<Vec<_>>()
                .join(r"\s+");
            if let Ok(regex) = regex::Regex::new(&pattern) {
                for regex_match in regex.find_iter(line.text) {
                    matches.push(EditMatch {
                        start: line.start + regex_match.start(),
                        end: line.start + regex_match.end(),
                    });
                }
            }
        }
    }

    matches.extend(find_line_based_edit_matches(
        content,
        old_string,
        normalize_whitespace,
    ));
    dedup_edit_matches(matches)
}

fn find_escape_normalized_edit_matches(content: &str, old_string: &str) -> Vec<EditMatch> {
    let unescaped_search = unescape_edit_string(old_string);
    let mut matches = content
        .match_indices(&unescaped_search)
        .map(|(start, value)| EditMatch {
            start,
            end: start + value.len(),
        })
        .collect::<Vec<_>>();

    let content_lines = split_lines_with_offsets(content);
    let search_lines = normalized_search_lines(&unescaped_search);
    if search_lines.is_empty() || search_lines.len() > content_lines.len() {
        return dedup_edit_matches(matches);
    }
    for start_line in 0..=content_lines.len() - search_lines.len() {
        let end_line = start_line + search_lines.len() - 1;
        let block = &content[content_lines[start_line].start..content_lines[end_line].end];
        if unescape_edit_string(block) == unescaped_search {
            matches.push(EditMatch {
                start: content_lines[start_line].start,
                end: content_lines[end_line].end,
            });
        }
    }
    dedup_edit_matches(matches)
}

fn find_trimmed_boundary_edit_matches(content: &str, old_string: &str) -> Vec<EditMatch> {
    let trimmed_search = old_string.trim();
    if trimmed_search == old_string || trimmed_search.is_empty() {
        return Vec::new();
    }

    let mut matches = content
        .match_indices(trimmed_search)
        .map(|(start, value)| EditMatch {
            start,
            end: start + value.len(),
        })
        .collect::<Vec<_>>();

    let content_lines = split_lines_with_offsets(content);
    let search_lines = normalized_search_lines(trimmed_search);
    if search_lines.is_empty() || search_lines.len() > content_lines.len() {
        return dedup_edit_matches(matches);
    }
    for start_line in 0..=content_lines.len() - search_lines.len() {
        let end_line = start_line + search_lines.len() - 1;
        let block = &content[content_lines[start_line].start..content_lines[end_line].end];
        if block.trim() == trimmed_search {
            matches.push(EditMatch {
                start: content_lines[start_line].start,
                end: content_lines[end_line].end,
            });
        }
    }
    dedup_edit_matches(matches)
}

fn find_context_aware_edit_matches(content: &str, old_string: &str) -> Vec<EditMatch> {
    let content_lines = split_lines_with_offsets(content);
    let search_lines = normalized_search_lines(old_string);
    if search_lines.len() < 3 {
        return Vec::new();
    }
    let first_line = search_lines[0].trim();
    let last_line = search_lines[search_lines.len() - 1].trim();
    if first_line.is_empty() || last_line.is_empty() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    for start_line in 0..content_lines.len() {
        if content_lines[start_line].text.trim() != first_line {
            continue;
        }
        for end_line in start_line + 2..content_lines.len() {
            if content_lines[end_line].text.trim() != last_line {
                continue;
            }
            let block_len = end_line - start_line + 1;
            if block_len != search_lines.len() {
                break;
            }
            if middle_lines_match_ratio(&content_lines, &search_lines, start_line, end_line) >= 0.5
            {
                matches.push(line_span_match(&content_lines, start_line, end_line));
            }
            break;
        }
    }
    matches
}

fn normalized_search_lines(text: &str) -> Vec<&str> {
    let mut lines = text.split('\n').collect::<Vec<_>>();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

fn line_span_match(lines: &[LineSpan<'_>], start_line: usize, end_line: usize) -> EditMatch {
    EditMatch {
        start: lines[start_line].start,
        end: lines[end_line].end,
    }
}

fn anchored_block_similarity(
    content_lines: &[LineSpan<'_>],
    search_lines: &[&str],
    start_line: usize,
    end_line: usize,
) -> f64 {
    let actual_middle = end_line.saturating_sub(start_line + 1);
    let search_middle = search_lines.len().saturating_sub(2);
    let lines_to_check = actual_middle.min(search_middle);
    if lines_to_check == 0 {
        return 1.0;
    }
    let mut similarity = 0.0;
    for offset in 0..lines_to_check {
        let original_line = content_lines[start_line + offset + 1].text.trim();
        let search_line = search_lines[offset + 1].trim();
        let max_len = original_line
            .chars()
            .count()
            .max(search_line.chars().count());
        if max_len == 0 {
            similarity += 1.0;
            continue;
        }
        let distance = levenshtein_chars(original_line, search_line);
        similarity += 1.0 - (distance as f64 / max_len as f64);
    }
    similarity / lines_to_check as f64
}

fn middle_lines_match_ratio(
    content_lines: &[LineSpan<'_>],
    search_lines: &[&str],
    start_line: usize,
    end_line: usize,
) -> f64 {
    let mut matching_lines = 0;
    let mut total_non_empty = 0;
    for (line, content_line) in content_lines
        .iter()
        .enumerate()
        .take(end_line)
        .skip(start_line + 1)
    {
        let offset = line - start_line;
        let content_line = content_line.text.trim();
        let search_line = search_lines[offset].trim();
        if content_line.is_empty() && search_line.is_empty() {
            continue;
        }
        total_non_empty += 1;
        if content_line == search_line {
            matching_lines += 1;
        }
    }
    if total_non_empty == 0 {
        1.0
    } else {
        matching_lines as f64 / total_non_empty as f64
    }
}

fn levenshtein_chars(left: &str, right: &str) -> usize {
    let left = left.chars().collect::<Vec<_>>();
    let right = right.chars().collect::<Vec<_>>();
    if left.is_empty() || right.is_empty() {
        return left.len().max(right.len());
    }
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    let mut current = vec![0; right.len() + 1];
    for (left_index, left_char) in left.iter().enumerate() {
        current[0] = left_index + 1;
        for (right_index, right_char) in right.iter().enumerate() {
            let insertion = current[right_index] + 1;
            let deletion = previous[right_index + 1] + 1;
            let substitution = previous[right_index] + usize::from(left_char != right_char);
            current[right_index + 1] = insertion.min(deletion).min(substitution);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

fn unescape_edit_string(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => output.push('\n'),
            Some('t') => output.push('\t'),
            Some('r') => output.push('\r'),
            Some('\'') => output.push('\''),
            Some('"') => output.push('"'),
            Some('`') => output.push('`'),
            Some('\\') => output.push('\\'),
            Some('\n') => output.push('\n'),
            Some('$') => output.push('$'),
            Some(other) => {
                output.push('\\');
                output.push(other);
            }
            None => output.push('\\'),
        }
    }
    output
}

fn dedup_edit_matches(matches: Vec<EditMatch>) -> Vec<EditMatch> {
    let mut deduped = Vec::new();
    for edit_match in matches {
        if !deduped.contains(&edit_match) {
            deduped.push(edit_match);
        }
    }
    deduped
}

#[derive(Debug, Clone, Copy)]
struct LineSpan<'a> {
    text: &'a str,
    start: usize,
    end: usize,
}

fn split_lines_with_offsets(content: &str) -> Vec<LineSpan<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for segment in content.split_inclusive('\n') {
        let end = start + segment.trim_end_matches('\n').len();
        lines.push(LineSpan {
            text: &content[start..end],
            start,
            end,
        });
        start += segment.len();
    }
    if start < content.len() || content.is_empty() {
        lines.push(LineSpan {
            text: &content[start..],
            start,
            end: content.len(),
        });
    }
    lines
}

fn strip_common_indentation(text: &str) -> String {
    let lines = text.split('\n').collect::<Vec<_>>();
    let min_indent = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            line.chars()
                .take_while(|ch| *ch == ' ' || *ch == '\t')
                .count()
        })
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                line.chars().skip(min_indent).collect::<String>()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) struct FileWriteOptions<'a> {
    pub(super) base_dir: &'a Path,
    pub(super) max_bytes: usize,
    pub(super) create_dirs: bool,
    pub(super) allow_overwrite: bool,
    pub(super) require_read: bool,
    pub(super) read_snapshots: &'a BTreeMap<PathBuf, SystemTime>,
}

pub(super) struct FileOpsOptions<'a> {
    pub(super) base_dir: &'a Path,
    pub(super) max_bytes: usize,
    pub(super) max_files: usize,
    pub(super) max_changed_lines: Option<usize>,
    pub(super) require_read: bool,
    pub(super) allow_new_files: bool,
    pub(super) allow_overwrite: bool,
    pub(super) allow_replace_all: bool,
    pub(super) read_snapshots: &'a BTreeMap<PathBuf, SystemTime>,
}

pub(super) struct FileEditOptions<'a> {
    pub(super) base_dir: &'a Path,
    pub(super) max_bytes: usize,
    pub(super) max_changed_lines: Option<usize>,
    pub(super) require_read: bool,
    pub(super) allow_replace_all: bool,
    pub(super) read_snapshots: &'a BTreeMap<PathBuf, SystemTime>,
}

pub(super) fn call_file_write_tool(
    name: &str,
    input: &Value,
    options: FileWriteOptions<'_>,
) -> Result<Value, RuntimeError> {
    let input_path = required_input_string(name, input, "path")?;
    validate_git_pathspec(name, input_path)?;
    let allowed_paths = allowed_paths_input(name, input)?;
    enforce_allowed_path(name, "input.path", input_path, &allowed_paths)?;
    let content = required_input_string(name, input, "content")?;
    let content_bytes = content.as_bytes();
    if content_bytes.len() > options.max_bytes {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.content exceeds max_bytes={}",
            options.max_bytes
        )));
    }
    let base = canonicalize_tool_path(name, "base_dir", options.base_dir)?;
    let candidate = base.join(input_path);
    let parent = candidate.parent().ok_or_else(|| {
        RuntimeError::Provider(format!(
            "tool {name} input.path must have a parent directory"
        ))
    })?;
    if options.create_dirs {
        fs::create_dir_all(parent).map_err(|error| {
            RuntimeError::Provider(format!("tool {name} create parent directories: {error}"))
        })?;
    }
    let parent = canonicalize_tool_path(name, "input.path parent", parent)?;
    if !parent.starts_with(&base) {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.path is outside configured base_dir"
        )));
    }
    let mut path = parent.join(candidate.file_name().ok_or_else(|| {
        RuntimeError::Provider(format!("tool {name} input.path must include a file name"))
    })?);
    let existed = path.exists();
    if existed {
        let existing = canonicalize_tool_path(name, "input.path", &path)?;
        if !existing.starts_with(&base) {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.path is outside configured base_dir"
            )));
        }
        if options.require_read {
            require_fresh_read(
                name,
                "input.path",
                "overwrite",
                &existing,
                options.read_snapshots,
            )?;
        }
        if !options.allow_overwrite {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.path already exists and allow_overwrite is false"
            )));
        }
        path = existing;
    }
    fs::write(&path, content_bytes)
        .map_err(|error| RuntimeError::Provider(format!("tool {name} write file: {error}")))?;
    Ok(json!({
        "path": path.display().to_string(),
        "bytes": content_bytes.len(),
        "created": !existed,
        "overwritten": existed,
        "artifacts": [{
            "id": format!("file-write:{}", path.display()),
            "kind": "file_write",
            "title": path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("file write"),
            "uri": path.display().to_string(),
            "content": format!("wrote {} bytes to {}", content_bytes.len(), path.display()),
            "metadata": {
                "provider": "file_write",
                "path": path.display().to_string(),
                "bytes": content_bytes.len(),
                "created": !existed,
                "overwritten": existed
            }
        }]
    }))
}

struct FileOpsPendingFile {
    path: String,
    absolute_path: PathBuf,
    kind: &'static str,
    content: String,
}

fn normalize_file_ops_input(input: &Value) -> Result<Value, RuntimeError> {
    if input.get("operations").is_some() {
        return normalize_file_ops_array_input(input, "operations");
    }
    if let Some(ops) = input.get("ops") {
        let mut normalized = serde_json::Map::new();
        normalized.insert(
            "operations".to_string(),
            normalize_file_ops_operations_with_default_path(
                ops,
                input.get("path").and_then(Value::as_str),
                input.get("kind").and_then(Value::as_str),
            )?,
        );
        if let Some(dry_run) = input.get("dry_run") {
            normalized.insert("dry_run".to_string(), dry_run.clone());
        }
        preserve_file_ops_scope_fields(input, &mut normalized);
        return Ok(Value::Object(normalized));
    }
    if let Some(edits) = input.get("edits") {
        let mut normalized = serde_json::Map::new();
        normalized.insert(
            "operations".to_string(),
            normalize_file_ops_operations_with_default_path(
                edits,
                input.get("path").and_then(Value::as_str),
                input.get("kind").and_then(Value::as_str),
            )?,
        );
        if let Some(dry_run) = input.get("dry_run") {
            normalized.insert("dry_run".to_string(), dry_run.clone());
        }
        preserve_file_ops_scope_fields(input, &mut normalized);
        return Ok(Value::Object(normalized));
    }
    let Some(kind) = input.get("kind").and_then(Value::as_str) else {
        return Ok(input.clone());
    };
    let mut operation = serde_json::Map::new();
    operation.insert("kind".to_string(), Value::String(kind.to_string()));
    for field in [
        "path",
        "old_string",
        "new_string",
        "content",
        "start_line",
        "end_line",
        "replace_all",
        "match_strategy",
    ] {
        if let Some(value) = input.get(field) {
            operation.insert(field.to_string(), value.clone());
        }
    }
    if let Some(args) = input.get("args").or_else(|| input.get("arguments")) {
        let Some(args) = args.as_object() else {
            return Ok(input.clone());
        };
        merge_file_ops_args(&mut operation, args);
    }
    normalize_file_ops_operation_aliases(&mut operation);
    let mut normalized = serde_json::Map::new();
    normalized.insert(
        "operations".to_string(),
        Value::Array(vec![Value::Object(operation)]),
    );
    if let Some(dry_run) = input.get("dry_run") {
        normalized.insert("dry_run".to_string(), dry_run.clone());
    }
    preserve_file_ops_scope_fields(input, &mut normalized);
    Ok(Value::Object(normalized))
}

fn normalize_file_ops_array_input(input: &Value, field: &str) -> Result<Value, RuntimeError> {
    let Some(operations) = input.get(field) else {
        return Ok(input.clone());
    };
    let mut normalized = input.clone();
    if let Some(object) = normalized.as_object_mut() {
        object.insert(
            "operations".to_string(),
            normalize_file_ops_operations_with_default_path(
                operations,
                input.get("path").and_then(Value::as_str),
                input.get("kind").and_then(Value::as_str),
            )?,
        );
    }
    Ok(normalized)
}

fn normalize_file_ops_operations_with_default_path(
    operations: &Value,
    default_path: Option<&str>,
    default_kind: Option<&str>,
) -> Result<Value, RuntimeError> {
    let Some(items) = operations.as_array() else {
        return Ok(operations.clone());
    };
    let mut normalized_items = Vec::with_capacity(items.len());
    for item in items {
        let Some(object) = item.as_object() else {
            normalized_items.push(item.clone());
            continue;
        };
        let mut operation = object.clone();
        if let Some(args) = object
            .get("args")
            .or_else(|| object.get("arguments"))
            .and_then(Value::as_object)
        {
            merge_file_ops_args(&mut operation, args);
        }
        if let Some(path) = default_path {
            operation
                .entry("path".to_string())
                .or_insert_with(|| Value::String(path.to_string()));
        }
        if let Some(kind) = default_kind {
            operation
                .entry("kind".to_string())
                .or_insert_with(|| Value::String(kind.to_string()));
        }
        normalize_file_ops_operation_aliases(&mut operation);
        normalized_items.push(Value::Object(operation));
    }
    Ok(Value::Array(normalized_items))
}

fn merge_file_ops_args(
    operation: &mut serde_json::Map<String, Value>,
    args: &serde_json::Map<String, Value>,
) {
    for (key, value) in args {
        operation
            .entry(key.to_string())
            .or_insert_with(|| value.clone());
    }
}

fn preserve_file_ops_scope_fields(input: &Value, normalized: &mut serde_json::Map<String, Value>) {
    if let Some(allowed_paths) = input.get("allowed_paths") {
        normalized.insert("allowed_paths".to_string(), allowed_paths.clone());
    }
}

fn allowed_paths_input(
    tool_name: &str,
    input: &Value,
) -> Result<Option<BTreeSet<String>>, RuntimeError> {
    let Some(allowed_paths) = input.get("allowed_paths") else {
        return Ok(None);
    };
    let Some(paths) = allowed_paths.as_array() else {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.allowed_paths must be an array of strings"
        )));
    };
    let mut normalized = BTreeSet::new();
    for (index, path) in paths.iter().enumerate() {
        let Some(path) = path.as_str() else {
            return Err(RuntimeError::Provider(format!(
                "tool {tool_name} input.allowed_paths[{index}] must be a string"
            )));
        };
        let path = path.trim();
        if path.is_empty() {
            continue;
        }
        validate_git_pathspec(tool_name, path)?;
        normalized.insert(path.to_string());
    }
    Ok(Some(normalized))
}

fn enforce_allowed_path(
    tool_name: &str,
    label: &str,
    path: &str,
    allowed_paths: &Option<BTreeSet<String>>,
) -> Result<(), RuntimeError> {
    let Some(allowed_paths) = allowed_paths else {
        return Ok(());
    };
    if allowed_paths.contains(path) {
        return Ok(());
    }
    let allowed = allowed_paths
        .iter()
        .map(|path| format!("'{path}'"))
        .collect::<Vec<_>>()
        .join(", ");
    Err(RuntimeError::Provider(format!(
        "tool {tool_name} {label} {path} is outside allowed_paths; allowed_paths=[{allowed}]"
    )))
}

fn repair_single_allowed_path(
    path: &str,
    allowed_paths: &Option<BTreeSet<String>>,
) -> Option<String> {
    let allowed_paths = allowed_paths.as_ref()?;
    if allowed_paths.contains(path) || allowed_paths.len() != 1 {
        return None;
    }
    allowed_paths.iter().next().cloned()
}

fn effective_max_changed_lines(
    tool_name: &str,
    input: &Value,
    configured_max: Option<usize>,
) -> Result<Option<usize>, RuntimeError> {
    let Some(configured_max) = configured_max else {
        return optional_positive_usize_input(tool_name, input, "max_changed_lines");
    };
    Ok(
        optional_bounded_usize_input(tool_name, input, "max_changed_lines", configured_max)?
            .or(Some(configured_max)),
    )
}

fn enforce_max_changed_lines(
    tool_name: &str,
    label: &str,
    diff: &str,
    max_changed_lines: Option<usize>,
) -> Result<(), RuntimeError> {
    let Some(max_changed_lines) = max_changed_lines else {
        return Ok(());
    };
    let changed_lines = count_changed_diff_lines(diff);
    if changed_lines <= max_changed_lines {
        return Ok(());
    }
    Err(RuntimeError::Provider(format!(
        "tool {tool_name} {label} changes {changed_lines} diff lines, exceeding max_changed_lines={max_changed_lines}"
    )))
}

fn count_changed_diff_lines(diff: &str) -> usize {
    diff.lines()
        .filter(|line| {
            (line.starts_with('+') && !line.starts_with("+++"))
                || (line.starts_with('-') && !line.starts_with("---"))
        })
        .count()
}

fn normalize_file_ops_operation_aliases(operation: &mut serde_json::Map<String, Value>) {
    if operation.get("kind").is_none()
        && operation.get("start_line").is_some()
        && operation.get("end_line").is_some()
        && (operation.get("content").is_some()
            || operation.get("new_lines").is_some()
            || operation.get("lines").is_some()
            || operation.get("replacement").is_some())
    {
        operation.insert(
            "kind".to_string(),
            Value::String("replace_lines".to_string()),
        );
    }
    let kind = operation.get("kind").and_then(Value::as_str);
    if kind != Some("replace_lines") || operation.contains_key("new_string") {
        return;
    }
    let value = if let Some(lines) = operation
        .get("new_lines")
        .or_else(|| operation.get("lines"))
        .and_then(Value::as_array)
    {
        let mut text = String::new();
        for line in lines {
            let Some(line) = line.as_str() else {
                return;
            };
            text.push_str(line);
            text.push('\n');
        }
        Some(Value::String(text))
    } else {
        operation
            .get("replacement")
            .or_else(|| operation.get("content"))
            .cloned()
    };
    let Some(mut value) = value else {
        return;
    };
    if let Some(text) = value.as_str() {
        if !text.ends_with('\n') {
            value = Value::String(format!("{text}\n"));
        }
    }
    operation.insert("new_string".to_string(), value);
}

fn labeled_string_input_with_aliases<'a>(
    name: &str,
    input: &'a Value,
    label: &str,
    field: &str,
    aliases: &[&str],
) -> Result<&'a str, RuntimeError> {
    if let Some(value) = optional_labeled_string_input(name, input, label, field)? {
        return Ok(value);
    }
    for alias in aliases {
        if let Some(value) = optional_labeled_string_input(name, input, label, alias)? {
            return Ok(value);
        }
    }
    Err(RuntimeError::Provider(format!(
        "tool {name} {label}.{field} must be a string"
    )))
}

pub(super) fn call_file_ops_tool(
    name: &str,
    input: &Value,
    options: FileOpsOptions<'_>,
) -> Result<Value, RuntimeError> {
    let normalized_input = normalize_file_ops_input(input)?;
    let input = &normalized_input;
    let dry_run = optional_bool_input(name, input, "dry_run")?.unwrap_or(false);
    let max_changed_lines = effective_max_changed_lines(name, input, options.max_changed_lines)?;
    let operations = input
        .get("operations")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            RuntimeError::Provider(format!("tool {name} input.operations must be an array"))
        })?;
    if operations.is_empty() {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.operations must contain at least one operation"
        )));
    }
    if operations.len() > options.max_files {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.operations contains {} operations, exceeding max_files={}",
            operations.len(),
            options.max_files
        )));
    }

    let base = canonicalize_tool_path(name, "base_dir", options.base_dir)?;
    let allowed_paths = allowed_paths_input(name, input)?;
    let mut pending = BTreeMap::<PathBuf, FileOpsPendingFile>::new();
    let mut diff = String::new();
    let mut match_strategies = Vec::new();
    let mut path_repairs = Vec::new();

    for (index, operation) in operations.iter().enumerate() {
        let label = format!("input.operations[{index}]");
        let Some(object) = operation.as_object() else {
            return Err(RuntimeError::Provider(format!(
                "tool {name} {label} must be an object"
            )));
        };
        let kind = required_labeled_string_input(name, operation, &label, "kind")?;
        let raw_input_path = required_labeled_string_input(name, operation, &label, "path")?;
        validate_git_pathspec(name, raw_input_path)?;
        let mut input_path = raw_input_path.to_string();
        if let Some(repaired_path) = repair_single_allowed_path(&input_path, &allowed_paths) {
            path_repairs.push(json!({
                "operation": index,
                "from": input_path.clone(),
                "to": repaired_path,
                "reason": "single_allowed_path"
            }));
            input_path = repaired_path;
        } else {
            enforce_allowed_path(name, &format!("{label}.path"), &input_path, &allowed_paths)?;
        }
        let candidate = base.join(&input_path);

        match kind {
            "edit" => {
                let path = canonicalize_tool_path(name, &format!("{label}.path"), &candidate)?;
                if !path.starts_with(&base) {
                    return Err(RuntimeError::Provider(format!(
                        "tool {name} {label}.path is outside configured base_dir"
                    )));
                }
                if options.require_read {
                    require_fresh_read(
                        name,
                        &format!("{label}.path"),
                        "edit",
                        &path,
                        options.read_snapshots,
                    )?;
                }
                let current = match pending.get(&path) {
                    Some(file) => file.content.clone(),
                    None => fs::read_to_string(&path).map_err(|error| {
                        RuntimeError::Provider(format!("tool {name} read file: {error}"))
                    })?,
                };
                let old_string =
                    required_labeled_string_input(name, operation, &label, "old_string")?;
                let new_string = labeled_string_input_with_aliases(
                    name,
                    operation,
                    &label,
                    "new_string",
                    &["replacement", "content"],
                )?;
                validate_file_edit_operation(name, &label, old_string, new_string)?;
                let replace_all =
                    optional_labeled_bool_input(name, operation, &label, "replace_all")?
                        .unwrap_or(false);
                if replace_all && !options.allow_replace_all {
                    return Err(RuntimeError::Provider(format!(
                        "tool {name} {label}.replace_all is true but allow_replace_all is false"
                    )));
                }
                let match_strategy = if object.get("match_strategy").is_some() {
                    EditMatchStrategy::from_labeled_input(name, operation, &label)?
                } else {
                    EditMatchStrategy::Auto
                };
                let (effective_match_strategy, matches) =
                    find_edit_matches(&current, old_string, match_strategy);
                if matches.is_empty() {
                    let anchor_line = old_string_anchor_line(&current, old_string);
                    return Ok(file_ops_failure_output(
                        name,
                        &base,
                        operations.len(),
                        &pending,
                        &diff,
                        FileOpsDiagnostic::new(
                            index,
                            label.clone(),
                            &input_path,
                            "edit",
                            "old_string",
                            format!(
                                "{label}.old_string was not found with match_strategy={}",
                                match_strategy.as_str()
                            ),
                        )
                        .with_match_strategy(match_strategy.as_str())
                        .with_match_count(0)
                        .with_anchor_line(anchor_line),
                    ));
                }
                if matches.len() > 1 && !replace_all {
                    return Ok(file_ops_failure_output(
                        name,
                        &base,
                        operations.len(),
                        &pending,
                        &diff,
                        FileOpsDiagnostic::new(
                            index,
                            label.clone(),
                            &input_path,
                            "edit",
                            "old_string",
                            format!(
                            "{label}.old_string matched {} times with match_strategy={}; set replace_all=true only when all matches should change",
                        matches.len(),
                        effective_match_strategy.as_str()
                            ),
                        )
                        .with_match_strategy(match_strategy.as_str())
                        .with_effective_match_strategy(effective_match_strategy.as_str())
                        .with_match_count(matches.len()),
                    ));
                }
                let selected = selected_edit_matches(&matches, replace_all);
                match_strategies.push(effective_match_strategy.as_str());
                diff.push_str(&edit_unified_diff(
                    &input_path,
                    &current,
                    &selected,
                    new_string,
                ));
                let updated = apply_selected_edit_matches(&current, &selected, new_string);
                if updated.len() > options.max_bytes {
                    return Ok(file_ops_failure_output(
                        name,
                        &base,
                        operations.len(),
                        &pending,
                        &diff,
                        FileOpsDiagnostic::new(
                            index,
                            label.clone(),
                            &input_path,
                            "edit",
                            "new_string",
                            format!(
                                "{label} edited content exceeds max_bytes={}",
                                options.max_bytes
                            ),
                        ),
                    ));
                }
                pending.insert(
                    path.clone(),
                    FileOpsPendingFile {
                        path: input_path.to_string(),
                        absolute_path: path,
                        kind: "existing",
                        content: updated,
                    },
                );
            }
            "replace_lines" => {
                let path = canonicalize_tool_path(name, &format!("{label}.path"), &candidate)?;
                if !path.starts_with(&base) {
                    return Err(RuntimeError::Provider(format!(
                        "tool {name} {label}.path is outside configured base_dir"
                    )));
                }
                if options.require_read {
                    require_fresh_read(
                        name,
                        &format!("{label}.path"),
                        "replace_lines",
                        &path,
                        options.read_snapshots,
                    )?;
                }
                let current = match pending.get(&path) {
                    Some(file) => file.content.clone(),
                    None => fs::read_to_string(&path).map_err(|error| {
                        RuntimeError::Provider(format!("tool {name} read file: {error}"))
                    })?,
                };
                let start_line =
                    required_labeled_usize_input(name, operation, &label, "start_line")?;
                let end_line = required_labeled_usize_input(name, operation, &label, "end_line")?;
                let new_string = labeled_string_input_with_aliases(
                    name,
                    operation,
                    &label,
                    "new_string",
                    &["replacement", "content"],
                )?;
                let Some(line_match) = line_range_edit_match(&current, start_line, end_line) else {
                    return Ok(file_ops_failure_output(
                        name,
                        &base,
                        operations.len(),
                        &pending,
                        &diff,
                        FileOpsDiagnostic::new(
                            index,
                            label.clone(),
                            &input_path,
                            "replace_lines",
                            "start_line",
                            format!(
                                "{label}.start_line/{label}.end_line must select an existing inclusive line range"
                            ),
                        ),
                    ));
                };
                let old_string = &current[line_match.start..line_match.end];
                validate_file_edit_operation(name, &label, old_string, new_string)?;
                let replacement = replace_lines_replacement(old_string, new_string);
                diff.push_str(&edit_unified_diff(
                    &input_path,
                    &current,
                    &[line_match],
                    &replacement,
                ));
                let updated = apply_selected_edit_matches(&current, &[line_match], &replacement);
                if updated.len() > options.max_bytes {
                    return Ok(file_ops_failure_output(
                        name,
                        &base,
                        operations.len(),
                        &pending,
                        &diff,
                        FileOpsDiagnostic::new(
                            index,
                            label.clone(),
                            &input_path,
                            "replace_lines",
                            "new_string",
                            format!(
                                "{label} edited content exceeds max_bytes={}",
                                options.max_bytes
                            ),
                        ),
                    ));
                }
                pending.insert(
                    path.clone(),
                    FileOpsPendingFile {
                        path: input_path.to_string(),
                        absolute_path: path,
                        kind: "existing",
                        content: updated,
                    },
                );
            }
            "write" => {
                let content = required_labeled_string_input(name, operation, &label, "content")?;
                if content.len() > options.max_bytes {
                    return Ok(file_ops_failure_output(
                        name,
                        &base,
                        operations.len(),
                        &pending,
                        &diff,
                        FileOpsDiagnostic::new(
                            index,
                            label.clone(),
                            &input_path,
                            "write",
                            "content",
                            format!("{label}.content exceeds max_bytes={}", options.max_bytes),
                        ),
                    ));
                }
                let (path, existed, old_content) =
                    resolve_file_ops_write_path(name, &label, &base, &input_path)?;
                if existed {
                    if !options.allow_overwrite {
                        return Ok(file_ops_failure_output(
                            name,
                            &base,
                            operations.len(),
                            &pending,
                            &diff,
                            FileOpsDiagnostic::new(
                                index,
                                label.clone(),
                                &input_path,
                                "write",
                                "path",
                                format!(
                                    "{label}.path already exists and allow_overwrite is false; use kind=edit or kind=replace_lines for existing files"
                                ),
                            ),
                        ));
                    }
                    if options.require_read {
                        require_fresh_read(
                            name,
                            &format!("{label}.path"),
                            "overwrite",
                            &path,
                            options.read_snapshots,
                        )?;
                    }
                } else if !options.allow_new_files {
                    return Ok(file_ops_failure_output(
                        name,
                        &base,
                        operations.len(),
                        &pending,
                        &diff,
                        FileOpsDiagnostic::new(
                            index,
                            label.clone(),
                            &input_path,
                            "write",
                            "path",
                            format!("{label}.path does not exist and allow_new_files is false"),
                        ),
                    ));
                }
                diff.push_str(&write_unified_diff(
                    &input_path,
                    old_content.as_deref(),
                    content,
                ));
                pending.insert(
                    path.clone(),
                    FileOpsPendingFile {
                        path: input_path.to_string(),
                        absolute_path: path,
                        kind: if existed { "existing" } else { "new" },
                        content: content.to_string(),
                    },
                );
            }
            _ => {
                return Err(RuntimeError::Provider(format!(
                    "tool {name} {label}.kind must be edit, replace_lines, or write"
                )));
            }
        }
    }

    if pending.len() > options.max_files {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.operations changes {} files, exceeding max_files={}",
            pending.len(),
            options.max_files
        )));
    }
    enforce_max_changed_lines(name, "input.operations", &diff, max_changed_lines)?;

    if !dry_run {
        for file in pending.values() {
            if let Some(parent) = file.absolute_path.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    RuntimeError::Provider(format!(
                        "tool {name} create parent directories for {}: {error}",
                        file.path
                    ))
                })?;
            }
            fs::write(&file.absolute_path, file.content.as_bytes()).map_err(|error| {
                RuntimeError::Provider(format!("tool {name} write {}: {error}", file.path))
            })?;
        }
    }

    let files = pending
        .values()
        .map(|file| {
            json!({
                "path": file.path,
                "kind": file.kind
            })
        })
        .collect::<Vec<_>>();
    let (diff_content, diff_truncated, diff_bytes) =
        bytes_to_limited_text(diff.as_bytes(), 64 * 1024);
    Ok(json!({
        "repo": base.display().to_string(),
        "success": true,
        "checked": true,
        "applied": !dry_run,
        "files": files,
        "file_count": pending.len(),
        "diagnostics": [],
        "match_strategies": match_strategies,
        "path_repairs": path_repairs,
        "bytes": diff_bytes,
        "diff": diff_content,
        "diff_truncated": diff_truncated,
        "artifacts": [{
            "id": format!("file-ops:{}:{}", base.display(), pending.len()),
            "kind": "file_ops",
            "title": format!("{} structured file operation(s)", operations.len()),
            "uri": base.display().to_string(),
            "content": diff_content.clone(),
            "metadata": {
                "provider": "file_ops",
                "repo": base.display().to_string(),
                "success": true,
                "checked": true,
                "applied": !dry_run,
                "dry_run": dry_run,
                "operation_count": operations.len(),
                "file_count": pending.len(),
                "match_strategies": match_strategies,
                "diff_bytes": diff_bytes,
                "diff_truncated": diff_truncated
            }
        }]
    }))
}

fn file_ops_failure_output(
    name: &str,
    base: &Path,
    operation_count: usize,
    pending: &BTreeMap<PathBuf, FileOpsPendingFile>,
    diff: &str,
    diagnostic: FileOpsDiagnostic,
) -> Value {
    let files = pending
        .values()
        .map(|file| {
            json!({
                "path": file.path,
                "kind": file.kind
            })
        })
        .collect::<Vec<_>>();
    let (diff_content, diff_truncated, diff_bytes) =
        bytes_to_limited_text(diff.as_bytes(), 64 * 1024);
    let diagnostic = diagnostic.into_json(name);
    json!({
        "repo": base.display().to_string(),
        "success": false,
        "checked": true,
        "applied": false,
        "files": files,
        "file_count": pending.len(),
        "diagnostics": [diagnostic],
        "bytes": diff_bytes,
        "diff": diff_content,
        "diff_truncated": diff_truncated,
        "artifacts": [{
            "id": format!("file-ops:{}:failed", base.display()),
            "kind": "file_ops",
            "title": "structured file operations failed validation",
            "uri": base.display().to_string(),
            "content": diff_content.clone(),
            "metadata": {
                "provider": "file_ops",
                "repo": base.display().to_string(),
                "success": false,
                "checked": true,
                "applied": false,
                "operation_count": operation_count,
                "file_count": pending.len(),
                "diff_bytes": diff_bytes,
                "diff_truncated": diff_truncated
            }
        }]
    })
}

fn required_labeled_usize_input(
    tool_name: &str,
    input: &Value,
    label: &str,
    field: &str,
) -> Result<usize, RuntimeError> {
    let Some(value) = input.get(field) else {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} {label}.{field} must be a positive integer"
        )));
    };
    let Some(number) = value.as_u64() else {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} {label}.{field} must be a positive integer"
        )));
    };
    if number == 0 {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} {label}.{field} must be greater than 0"
        )));
    }
    usize::try_from(number).map_err(|_| {
        RuntimeError::Provider(format!("tool {tool_name} {label}.{field} is too large"))
    })
}

fn line_range_edit_match(content: &str, start_line: usize, end_line: usize) -> Option<EditMatch> {
    if start_line == 0 || end_line < start_line || content.is_empty() {
        return None;
    }
    let lines = split_lines_with_offsets(content);
    if start_line > lines.len() || end_line > lines.len() {
        return None;
    }
    let end = if end_line < lines.len() {
        lines[end_line].start
    } else {
        content.len()
    };
    Some(EditMatch {
        start: lines[start_line - 1].start,
        end,
    })
}

fn replace_lines_replacement(old_string: &str, new_string: &str) -> String {
    if old_string.ends_with('\n') && !new_string.ends_with('\n') {
        format!("{new_string}\n")
    } else {
        new_string.to_string()
    }
}

#[derive(Debug, Clone)]
struct FileOpsDiagnostic {
    operation_index: usize,
    operation_label: String,
    path: String,
    kind: String,
    field: String,
    message: String,
    match_strategy: Option<String>,
    effective_match_strategy: Option<String>,
    match_count: Option<usize>,
    line: Option<usize>,
    line_source: Option<String>,
}

impl FileOpsDiagnostic {
    fn new(
        operation_index: usize,
        operation_label: String,
        path: &str,
        kind: &str,
        field: &str,
        message: String,
    ) -> Self {
        Self {
            operation_index,
            operation_label,
            path: path.to_string(),
            kind: kind.to_string(),
            field: field.to_string(),
            message,
            match_strategy: None,
            effective_match_strategy: None,
            match_count: None,
            line: None,
            line_source: None,
        }
    }

    fn with_match_strategy(mut self, strategy: &str) -> Self {
        self.match_strategy = Some(strategy.to_string());
        self
    }

    fn with_effective_match_strategy(mut self, strategy: &str) -> Self {
        self.effective_match_strategy = Some(strategy.to_string());
        self
    }

    fn with_match_count(mut self, count: usize) -> Self {
        self.match_count = Some(count);
        self
    }

    fn with_anchor_line(mut self, line: Option<usize>) -> Self {
        if let Some(line) = line {
            self.line = Some(line);
            self.line_source = Some("old_string_anchor".to_string());
        }
        self
    }

    fn into_json(self, source: &str) -> Value {
        json!({
            "source": source,
            "severity": "error",
            "message": self.message,
            "operation_index": self.operation_index,
            "operation_label": self.operation_label,
            "path": self.path,
            "kind": self.kind,
            "field": self.field,
            "match_strategy": self.match_strategy,
            "effective_match_strategy": self.effective_match_strategy,
            "match_count": self.match_count,
            "line": self.line,
            "line_source": self.line_source
        })
    }
}

fn old_string_anchor_line(content: &str, old_string: &str) -> Option<usize> {
    old_string
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            let matches = content
                .lines()
                .enumerate()
                .filter_map(|(index, content_line)| {
                    (content_line.trim() == trimmed).then_some(index + 1)
                })
                .collect::<Vec<_>>();
            if matches.len() == 1 {
                Some((matches[0], trimmed.len()))
            } else {
                None
            }
        })
        .max_by_key(|(_, len)| *len)
        .map(|(line, _)| line)
}

fn resolve_file_ops_write_path(
    name: &str,
    label: &str,
    base: &Path,
    input_path: &str,
) -> Result<(PathBuf, bool, Option<String>), RuntimeError> {
    let candidate = base.join(input_path);
    if candidate.exists() {
        let path = canonicalize_tool_path(name, &format!("{label}.path"), &candidate)?;
        if !path.starts_with(base) {
            return Err(RuntimeError::Provider(format!(
                "tool {name} {label}.path is outside configured base_dir"
            )));
        }
        let content = fs::read_to_string(&path).map_err(|error| {
            RuntimeError::Provider(format!("tool {name} read existing file: {error}"))
        })?;
        return Ok((path, true, Some(content)));
    }

    let parent = candidate.parent().ok_or_else(|| {
        RuntimeError::Provider(format!(
            "tool {name} {label}.path must have a parent directory"
        ))
    })?;
    let parent = canonicalize_tool_path(name, &format!("{label}.path parent"), parent)?;
    if !parent.starts_with(base) {
        return Err(RuntimeError::Provider(format!(
            "tool {name} {label}.path is outside configured base_dir"
        )));
    }
    let file_name = candidate.file_name().ok_or_else(|| {
        RuntimeError::Provider(format!("tool {name} {label}.path must include a file name"))
    })?;
    Ok((parent.join(file_name), false, None))
}

fn write_unified_diff(path: &str, old_content: Option<&str>, new_content: &str) -> String {
    let old = old_content.unwrap_or("");
    let mut diff = if old_content.is_some() {
        format!("--- a/{path}\n+++ b/{path}\n")
    } else {
        format!("--- /dev/null\n+++ b/{path}\n")
    };
    diff.push_str(&format!(
        "@@ -{},{} +{},{} @@\n",
        if old_content.is_some() { 1 } else { 0 },
        if old_content.is_some() {
            diff_line_count(old)
        } else {
            0
        },
        1,
        diff_line_count(new_content)
    ));
    for line in diff_lines(old) {
        diff.push('-');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in diff_lines(new_content) {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    diff
}

fn parse_file_edit_operations<'a>(
    name: &str,
    input: &'a Value,
    allow_replace_all: bool,
) -> Result<Vec<FileEditOperation<'a>>, RuntimeError> {
    let global_replace_all = optional_bool_input(name, input, "replace_all")?.unwrap_or(false);
    if global_replace_all && !allow_replace_all {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.replace_all is true but allow_replace_all is false"
        )));
    }
    let global_match_strategy = EditMatchStrategy::from_input(name, input)?;

    if let Some(edits_value) = input.get("edits") {
        if input.get("old_string").is_some() || input.get("new_string").is_some() {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input must use either old_string/new_string or edits, not both"
            )));
        }
        let edits = edits_value.as_array().ok_or_else(|| {
            RuntimeError::Provider(format!("tool {name} input.edits must be an array"))
        })?;
        if edits.is_empty() {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.edits must contain at least one edit"
            )));
        }
        if edits.len() > MAX_FILE_EDIT_OPERATIONS {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.edits must contain at most {MAX_FILE_EDIT_OPERATIONS} edits"
            )));
        }
        let mut operations = Vec::with_capacity(edits.len());
        for (index, edit) in edits.iter().enumerate() {
            let label = format!("input.edits[{index}]");
            if !edit.is_object() {
                return Err(RuntimeError::Provider(format!(
                    "tool {name} {label} must be an object"
                )));
            }
            let old_string = required_labeled_string_input(name, edit, &label, "old_string")?;
            let new_string = required_labeled_string_input(name, edit, &label, "new_string")?;
            let replace_all = optional_labeled_bool_input(name, edit, &label, "replace_all")?
                .unwrap_or(global_replace_all);
            if replace_all && !allow_replace_all {
                return Err(RuntimeError::Provider(format!(
                    "tool {name} {label}.replace_all is true but allow_replace_all is false"
                )));
            }
            let match_strategy = if edit.get("match_strategy").is_some() {
                EditMatchStrategy::from_labeled_input(name, edit, &label)?
            } else {
                global_match_strategy
            };
            validate_file_edit_operation(name, &label, old_string, new_string)?;
            operations.push(FileEditOperation {
                label,
                old_string,
                new_string,
                replace_all,
                match_strategy,
            });
        }
        return Ok(operations);
    }

    let old_string = required_input_string(name, input, "old_string")?;
    let new_string = required_input_string(name, input, "new_string")?;
    validate_file_edit_operation(name, "input", old_string, new_string)?;
    Ok(vec![FileEditOperation {
        label: "input".to_string(),
        old_string,
        new_string,
        replace_all: global_replace_all,
        match_strategy: global_match_strategy,
    }])
}

fn validate_file_edit_operation(
    name: &str,
    label: &str,
    old_string: &str,
    new_string: &str,
) -> Result<(), RuntimeError> {
    if old_string.is_empty() {
        return Err(RuntimeError::Provider(format!(
            "tool {name} {label}.old_string must not be empty"
        )));
    }
    if old_string == new_string {
        return Err(RuntimeError::Provider(format!(
            "tool {name} {label}.new_string must be different from {label}.old_string"
        )));
    }
    Ok(())
}

pub(super) fn call_file_edit_tool(
    name: &str,
    input: &Value,
    options: FileEditOptions<'_>,
) -> Result<Value, RuntimeError> {
    let input_path = required_input_string(name, input, "path")?;
    validate_git_pathspec(name, input_path)?;
    let allowed_paths = allowed_paths_input(name, input)?;
    enforce_allowed_path(name, "input.path", input_path, &allowed_paths)?;
    let operations = parse_file_edit_operations(name, input, options.allow_replace_all)?;
    let dry_run = optional_bool_input(name, input, "dry_run")?.unwrap_or(false);
    let max_changed_lines = effective_max_changed_lines(name, input, options.max_changed_lines)?;

    let base = canonicalize_tool_path(name, "base_dir", options.base_dir)?;
    let candidate = base.join(input_path);
    let path = canonicalize_tool_path(name, "input.path", &candidate)?;
    if !path.starts_with(&base) {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.path is outside configured base_dir"
        )));
    }
    if options.require_read {
        require_fresh_read(name, "input.path", "edit", &path, options.read_snapshots)?;
    }

    let content = fs::read_to_string(&path)
        .map_err(|error| RuntimeError::Provider(format!("tool {name} read file: {error}")))?;
    let mut updated = content.clone();
    let mut total_replacements = 0usize;
    let mut diff = String::new();
    let mut strategies = Vec::new();
    for operation in &operations {
        let (effective_match_strategy, matches) =
            find_edit_matches(&updated, operation.old_string, operation.match_strategy);
        if matches.is_empty() {
            return Err(RuntimeError::Provider(format!(
                "tool {name} {}.old_string was not found with match_strategy={}",
                operation.label,
                operation.match_strategy.as_str()
            )));
        }
        if matches.len() > 1 && !operation.replace_all {
            return Err(RuntimeError::Provider(format!(
                "tool {name} {}.old_string matched {} times with match_strategy={}; set replace_all=true only when all matches should change",
                operation.label,
                matches.len(),
                effective_match_strategy.as_str()
            )));
        }
        let selected_matches = selected_edit_matches(&matches, operation.replace_all);
        diff.push_str(&edit_unified_diff(
            input_path,
            &updated,
            &selected_matches,
            operation.new_string,
        ));
        total_replacements += selected_matches.len();
        strategies.push(effective_match_strategy.as_str());
        updated = apply_selected_edit_matches(&updated, &selected_matches, operation.new_string);
    }

    let updated_bytes = updated.as_bytes();
    if updated_bytes.len() > options.max_bytes {
        return Err(RuntimeError::Provider(format!(
            "tool {name} edited content exceeds max_bytes={}",
            options.max_bytes
        )));
    }
    let (diff_content, diff_truncated, diff_bytes) =
        bytes_to_limited_text(diff.as_bytes(), 64 * 1024);
    enforce_max_changed_lines(name, "input", &diff, max_changed_lines)?;
    if !dry_run {
        fs::write(&path, updated_bytes)
            .map_err(|error| RuntimeError::Provider(format!("tool {name} write file: {error}")))?;
    }

    Ok(json!({
        "repo": base.display().to_string(),
        "path": path.display().to_string(),
        "success": true,
        "checked": true,
        "applied": !dry_run,
        "files": [{
            "path": input_path,
            "kind": "existing"
        }],
        "file_count": 1,
        "diagnostics": [],
        "bytes": updated_bytes.len(),
        "replacements": total_replacements,
        "edit_count": operations.len(),
        "match_strategy": strategies[0],
        "match_strategies": strategies,
        "diff": diff_content,
        "diff_truncated": diff_truncated,
        "artifacts": [{
            "id": format!("file-edit:{}", path.display()),
            "kind": "file_edit",
            "title": path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("file edit"),
            "uri": path.display().to_string(),
            "content": diff_content.clone(),
            "metadata": {
                "provider": "file_edit",
                "path": path.display().to_string(),
                "repo": base.display().to_string(),
                "success": true,
                "checked": true,
                "applied": !dry_run,
                "dry_run": dry_run,
                "bytes": updated_bytes.len(),
                "replacements": total_replacements,
                "edit_count": operations.len(),
                "match_strategies": strategies,
                "diff_bytes": diff_bytes,
                "diff_truncated": diff_truncated,
                "summary": format!(
                    "edited {} replacement(s) across {} operation(s) in {}",
                    total_replacements,
                    operations.len(),
                    path.display(),
                )
            }
        }]
    }))
}

pub(super) struct FilePatchOptions<'a> {
    pub(super) repo_dir: &'a Path,
    pub(super) max_bytes: usize,
    pub(super) max_files: usize,
    pub(super) max_changed_lines: Option<usize>,
    pub(super) require_read: bool,
    pub(super) allow_new_files: bool,
    pub(super) allow_delete_files: bool,
    pub(super) read_snapshots: &'a BTreeMap<PathBuf, SystemTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatchPathKind {
    Existing,
    New,
    Delete,
}

pub(super) fn call_file_patch_tool(
    name: &str,
    input: &Value,
    options: FilePatchOptions<'_>,
) -> Result<Value, RuntimeError> {
    let patch = required_input_string(name, input, "patch")?;
    let patch = patch.replace("\r\n", "\n").replace('\r', "\n");
    let dry_run = optional_bool_input(name, input, "dry_run")?.unwrap_or(false);
    if patch.trim().is_empty() {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.patch must not be empty"
        )));
    }
    if patch.len() > options.max_bytes {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.patch exceeds max_bytes={}",
            options.max_bytes
        )));
    }
    let max_changed_lines = effective_max_changed_lines(name, input, options.max_changed_lines)?;
    enforce_max_changed_lines(name, "input.patch", &patch, max_changed_lines)?;

    let repo = canonicalize_tool_path(name, "repo_dir", options.repo_dir)?;
    let changed = extract_patch_paths(name, &patch)?;
    let allowed_paths = allowed_paths_input(name, input)?;
    if changed.is_empty() {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.patch did not declare any changed files"
        )));
    }
    if changed.len() > options.max_files {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.patch changes {} files, exceeding max_files={}",
            changed.len(),
            options.max_files
        )));
    }

    for (path, kind) in &changed {
        enforce_allowed_path(name, "input.patch path", path, &allowed_paths)?;
        if *kind == PatchPathKind::New && !options.allow_new_files {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.patch creates {path}, but allow_new_files is false"
            )));
        }
        if *kind == PatchPathKind::Delete && !options.allow_delete_files {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.patch deletes {path}, but allow_delete_files is false"
            )));
        }
        validate_git_pathspec(name, path)?;
        let candidate = repo.join(path);
        if candidate.exists() {
            let existing = canonicalize_tool_path(name, "input.patch path", &candidate)?;
            if !existing.starts_with(&repo) {
                return Err(RuntimeError::Provider(format!(
                    "tool {name} input.patch path is outside configured repo_dir"
                )));
            }
            if options.require_read {
                require_fresh_read(
                    name,
                    &format!("input.patch path {path}"),
                    "patch",
                    &existing,
                    options.read_snapshots,
                )?;
            }
        } else if *kind != PatchPathKind::New {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.patch references missing file {path}"
            )));
        }
    }

    let patch_file = write_temp_patch_file(name, &patch)?;
    let check = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .arg("apply")
        .arg("--check")
        .arg(&patch_file)
        .output()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} git apply --check: {error}")));
    let check = match check {
        Ok(output) => output,
        Err(error) => {
            let _ = fs::remove_file(&patch_file);
            return Err(error);
        }
    };
    if !check.status.success() {
        let diagnostic = patch_diagnostic_from_stderr(&String::from_utf8_lossy(&check.stderr));
        let _ = fs::remove_file(&patch_file);
        if dry_run {
            return Ok(file_patch_output(
                &repo,
                &changed,
                patch,
                false,
                true,
                false,
                vec![diagnostic],
            ));
        }
        return Err(RuntimeError::Provider(format!(
            "tool {name} git apply --check failed: {}",
            provider_error_snippet(&String::from_utf8_lossy(&check.stderr))
        )));
    }

    if dry_run {
        let _ = fs::remove_file(&patch_file);
        return Ok(file_patch_output(
            &repo,
            &changed,
            patch,
            true,
            true,
            false,
            Vec::new(),
        ));
    }

    let apply = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .arg("apply")
        .arg(&patch_file)
        .output()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} git apply: {error}")));
    let _ = fs::remove_file(&patch_file);
    let apply = apply?;
    if !apply.status.success() {
        return Err(RuntimeError::Provider(format!(
            "tool {name} git apply failed after successful check: {}",
            provider_error_snippet(&String::from_utf8_lossy(&apply.stderr))
        )));
    }

    Ok(file_patch_output(
        &repo,
        &changed,
        patch,
        true,
        true,
        true,
        Vec::new(),
    ))
}

fn file_patch_output(
    repo: &Path,
    changed: &BTreeMap<String, PatchPathKind>,
    patch: String,
    success: bool,
    checked: bool,
    applied: bool,
    diagnostics: Vec<Value>,
) -> Value {
    let diagnostics_count = diagnostics.len();
    let patch_bytes = patch.len();
    let files = changed
        .iter()
        .map(|(path, kind)| {
            json!({
                "path": path,
                "kind": match kind {
                    PatchPathKind::Existing => "existing",
                    PatchPathKind::New => "new",
                    PatchPathKind::Delete => "delete",
                }
            })
        })
        .collect::<Vec<_>>();
    json!({
        "repo": repo.display().to_string(),
        "success": success,
        "checked": checked,
        "applied": applied,
        "files": files,
        "file_count": changed.len(),
        "bytes": patch_bytes,
        "diagnostics": diagnostics,
        "artifacts": [{
            "id": format!("file-patch:{}:{}", repo.display(), changed.keys().cloned().collect::<Vec<_>>().join(",")),
            "kind": "file_patch",
            "title": "file patch",
            "uri": repo.display().to_string(),
            "content": patch,
            "metadata": {
                "provider": "file_patch",
                "repo": repo.display().to_string(),
                "file_count": changed.len(),
                "success": success,
                "checked": checked,
                "applied": applied,
                "diagnostics_count": diagnostics_count
            }
        }]
    })
}

fn patch_diagnostic_from_stderr(stderr: &str) -> Value {
    let message = provider_error_snippet(stderr);
    json!({
        "source": "file_patch",
        "severity": "error",
        "message": message,
        "raw": message
    })
}

fn extract_patch_paths(
    name: &str,
    patch: &str,
) -> Result<BTreeMap<String, PatchPathKind>, RuntimeError> {
    let mut paths = BTreeMap::new();
    let mut pending_old_is_null = false;
    let mut pending_old_path = None;
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            let parts = rest.split_whitespace().collect::<Vec<_>>();
            if parts.len() != 2 {
                return Err(RuntimeError::Provider(format!(
                    "tool {name} input.patch has unsupported diff --git header"
                )));
            }
            let old_path = patch_path_from_token(name, parts[0], "a/")?;
            let new_path = patch_path_from_token(name, parts[1], "b/")?;
            if old_path != "/dev/null" {
                paths.entry(old_path).or_insert(PatchPathKind::Existing);
            }
            if new_path != "/dev/null" {
                paths.entry(new_path).or_insert(PatchPathKind::Existing);
            }
        } else if let Some(rest) = line.strip_prefix("--- ") {
            let token = rest.split_whitespace().next().unwrap_or_default();
            pending_old_is_null = token == "/dev/null";
            if !pending_old_is_null {
                let path = patch_path_from_token(name, token, "a/")?;
                pending_old_path = Some(path.clone());
                paths.entry(path).or_insert(PatchPathKind::Existing);
            } else {
                pending_old_path = None;
            }
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            let token = rest.split_whitespace().next().unwrap_or_default();
            if token == "/dev/null" {
                let Some(path) = pending_old_path.take() else {
                    return Err(RuntimeError::Provider(format!(
                        "tool {name} input.patch deletes a file without a path"
                    )));
                };
                paths.insert(path, PatchPathKind::Delete);
            } else {
                let path = patch_path_from_token(name, token, "b/")?;
                let kind = if pending_old_is_null {
                    PatchPathKind::New
                } else {
                    PatchPathKind::Existing
                };
                paths.insert(path, kind);
            }
            pending_old_is_null = false;
            pending_old_path = None;
        }
    }
    Ok(paths)
}

fn patch_path_from_token(
    name: &str,
    token: &str,
    expected_prefix: &str,
) -> Result<String, RuntimeError> {
    if token == "/dev/null" {
        return Ok(token.to_string());
    }
    if token.starts_with('"') || token.contains('\\') {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.patch paths must be unquoted relative paths without escapes"
        )));
    }
    let Some(path) = token.strip_prefix(expected_prefix) else {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.patch paths must use {expected_prefix} prefixes"
        )));
    };
    validate_git_pathspec(name, path)?;
    Ok(path.to_string())
}

fn write_temp_patch_file(name: &str, patch: &str) -> Result<PathBuf, RuntimeError> {
    let temp_dir = std::env::temp_dir();
    let process_id = std::process::id();
    for attempt in 0..1024u16 {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| RuntimeError::Provider(format!("tool {name} system clock: {error}")))?
            .as_nanos();
        let path = temp_dir.join(format!(
            "air-tool-patch-{process_id}-{nonce}-{attempt}.patch"
        ));
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(RuntimeError::Provider(format!(
                    "tool {name} write temp patch: {error}"
                )));
            }
        };
        file.write_all(patch.as_bytes()).map_err(|error| {
            let _ = fs::remove_file(&path);
            RuntimeError::Provider(format!("tool {name} write temp patch: {error}"))
        })?;
        return Ok(path);
    }
    Err(RuntimeError::Provider(format!(
        "tool {name} failed to allocate a unique temp patch file"
    )))
}
