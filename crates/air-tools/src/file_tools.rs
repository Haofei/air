use super::*;

const DEFAULT_UNSCOPED_READ_LINE_LIMIT: usize = 2000;

pub(super) fn call_file_read_tool(
    name: &str,
    input: &Value,
    base_dir: &Path,
    max_bytes: usize,
) -> Result<Value, RuntimeError> {
    let input_path = required_path_input(name, input)?;
    let base = canonicalize_tool_path(name, "base_dir", base_dir)?;
    let candidate = if Path::new(&input_path).is_absolute() {
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
    let offset = optional_bounded_zero_or_positive_usize_input(name, input, "offset", usize::MAX)?;
    let limit = optional_positive_usize_input(name, input, "limit")?;
    if lines_range.is_some()
        && (input.get("start_line").is_some()
            || input.get("end_line").is_some()
            || offset.is_some()
            || limit.is_some())
    {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.lines cannot be combined with input.start_line, input.end_line, input.offset, or input.limit"
        )));
    }
    let explicit_start_line = optional_positive_usize_input(name, input, "start_line")?;
    let explicit_end_line = optional_positive_usize_input(name, input, "end_line")?;
    if offset.is_some() && (explicit_start_line.is_some() || explicit_end_line.is_some()) {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.offset cannot be combined with input.start_line or input.end_line"
        )));
    }
    let (start_line, end_line) = if explicit_start_line.is_some() || explicit_end_line.is_some() {
        let start = explicit_start_line;
        let end = explicit_end_line.or_else(|| {
            start.and_then(|start| limit.map(|limit| start.saturating_add(limit).saturating_sub(1)))
        });
        (start, end)
    } else if let Some(offset) = offset {
        let start = offset.saturating_add(1);
        let end = limit.map(|limit| start.saturating_add(limit).saturating_sub(1));
        (Some(start), end)
    } else if let Some(limit) = limit {
        (Some(1), Some(limit))
    } else if let Some((start, end)) = lines_range {
        (Some(start), Some(end))
    } else {
        (None, None)
    };
    let contains = optional_string_input(name, input, "contains")?;
    let context_lines = optional_positive_usize_input(name, input, "context_lines")?.unwrap_or(0);
    let occurrence = optional_positive_usize_input(name, input, "occurrence")?.unwrap_or(1);
    let explicit_line_numbers = input.get("line_numbers").is_some();
    let line_numbers = optional_bool_input(name, input, "line_numbers")?.unwrap_or(true);
    let effective_max_bytes =
        optional_bounded_usize_input(name, input, "max_bytes", max_bytes)?.unwrap_or(max_bytes);
    if let (Some(start), Some(end)) = (start_line, end_line) {
        if start > end {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.start_line must be less than or equal to input.end_line"
            )));
        }
    }
    let range_limited_unscoped_read = contains.is_none()
        && start_line.is_none()
        && end_line.is_none()
        && total_lines > DEFAULT_UNSCOPED_READ_LINE_LIMIT;
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
        if range_limited_unscoped_read {
            (Some(1), Some(DEFAULT_UNSCOPED_READ_LINE_LIMIT), None)
        } else {
            (start_line, end_line, None)
        }
    };
    let selected = select_line_range(&full_content, effective_start_line, effective_end_line);
    let (selected_content, truncated, bytes, full_output_path) = limit_and_maybe_save(
        name,
        &base,
        "file-read",
        selected.as_bytes(),
        effective_max_bytes,
    )?;
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
        range_limited_unscoped_read,
        body.len(),
        total_lines,
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
            "range_limited_unscoped_read": range_limited_unscoped_read,
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
        "range_limited_unscoped_read": range_limited_unscoped_read,
        "line_numbers": line_numbers,
        "line_numbers_defaulted": !explicit_line_numbers && line_numbers,
        "artifacts": [artifact],
    }))
}

fn file_read_truncation_hint(
    truncated: bool,
    unscoped_read: bool,
    range_limited_unscoped_read: bool,
    source_bytes: usize,
    total_lines: usize,
    full_output_path: Option<&str>,
) -> Option<String> {
    if range_limited_unscoped_read {
        return Some(format!(
            "Unscoped read was limited to the first {DEFAULT_UNSCOPED_READ_LINE_LIMIT} of {total_lines} lines. Use offset+limit, start_line/end_line, or contains+context_lines to inspect the relevant range."
        ));
    }
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
    let (pattern, pattern_source) = file_search_pattern_input(name, input)?;
    if pattern.is_empty() {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.pattern must not be empty"
        )));
    }
    let regex = regex::Regex::new(pattern)
        .map_err(|error| RuntimeError::Provider(format!("tool {name} invalid regex: {error}")))?;
    let base = canonicalize_tool_path(name, "base_dir", base_dir)?;
    let (input_path, include_glob) = file_search_path_input(name, input, &base)?;
    let input_path = input_path.unwrap_or_else(|| ".".to_string());
    let candidate = if Path::new(&input_path).is_absolute() {
        PathBuf::from(&input_path)
    } else {
        base.join(&input_path)
    };
    let path = canonicalize_tool_path(name, "input.path", &candidate)?;
    if !path.starts_with(&base) {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.path is outside configured base_dir"
        )));
    }
    let context_lines = optional_bounded_zero_or_positive_usize_alias_input(
        name,
        input,
        "context_lines",
        "contextLines",
        max_context_lines,
    )?
    .unwrap_or(0);
    let effective_max_matches =
        optional_bounded_usize_alias_input(name, input, "max_matches", "maxMatches", max_matches)?
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
            if let Some(include_glob) = &include_glob {
                if !file_search_glob_matches(include_glob, &relative_path) {
                    continue;
                }
            }
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
    let (artifact_content, content_truncated, bytes, full_output_path) =
        limit_and_maybe_save(name, &base, "file-search", rendered.as_bytes(), max_bytes)?;
    let match_truncated = result.total_match_count > result.matches.len();
    let truncated = match_truncated || content_truncated || result.any_line_truncated;
    let truncation_hint = file_search_truncation_hint(
        content_truncated,
        match_truncated,
        full_output_path.as_deref(),
    );
    let no_matches = result.total_match_count == 0;
    let search_hint =
        no_matches.then(|| file_search_no_matches_hint(directory, include_glob.as_deref()));
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
            "include_glob": include_glob,
            "match_count": result.total_match_count,
            "returned_match_count": result.matches.len(),
            "no_matches": no_matches,
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
            "search_hint": search_hint
        }
    });
    Ok(json!({
        "path": path.display().to_string(),
        "directory": directory,
        "pattern": pattern,
        "pattern_source": pattern_source,
        "include_glob": include_glob,
        "matches": result.matches,
        "match_count": result.total_match_count,
        "returned_match_count": result.matches.len(),
        "no_matches": no_matches,
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
        "search_hint": search_hint,
        "bytes": bytes,
        "artifacts": [artifact]
    }))
}

fn file_search_no_matches_hint(directory: bool, include_glob: Option<&str>) -> String {
    let scope = if directory {
        if include_glob.is_some() {
            "No matches in the current directory/glob scope."
        } else {
            "No matches in the current directory scope."
        }
    } else {
        "No matches in this file."
    };
    format!("{scope} broaden the pattern/path, inspect the repo file list, or switch to a likely source/test extension instead of repeating the same search.")
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

fn limit_and_maybe_save(
    tool_name: &str,
    base_dir: &Path,
    prefix: &str,
    content: &[u8],
    max_bytes: usize,
) -> Result<(String, bool, usize, Option<String>), RuntimeError> {
    let (limited, truncated, bytes) = bytes_to_limited_text(content, max_bytes);
    let full_output_path = maybe_save_full_output(tool_name, base_dir, prefix, content, truncated)?;
    Ok((limited, truncated, bytes, full_output_path))
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

fn file_search_path_input(
    tool_name: &str,
    input: &Value,
    base: &Path,
) -> Result<(Option<String>, Option<String>), RuntimeError> {
    if let Some(path) = optional_path_input(tool_name, input)? {
        return Ok((Some(path.to_string()), None));
    }
    let Some(value) = input.get("include") else {
        return Ok((None, None));
    };
    let include = value.as_str().ok_or_else(|| {
        RuntimeError::Provider(format!("tool {tool_name} input.include must be a string"))
    })?;
    if include.trim().is_empty() {
        return Ok((None, None));
    }
    let candidate = if Path::new(include).is_absolute() {
        PathBuf::from(include)
    } else {
        base.join(include)
    };
    if candidate.exists() {
        return Ok((Some(include.to_string()), None));
    }
    super::validate_relative_path_filter(tool_name, include)?;
    Ok((Some(".".to_string()), Some(include.to_string())))
}

fn file_search_glob_matches(glob: &str, relative_path: &str) -> bool {
    let glob = glob.replace('\\', "/");
    let relative_path = relative_path.replace('\\', "/");
    if file_search_wildcard_matches(&glob, &relative_path) {
        return true;
    }
    if glob.contains('/') {
        return false;
    }
    relative_path
        .rsplit('/')
        .next()
        .is_some_and(|file_name| file_search_wildcard_matches(&glob, file_name))
}

fn file_search_wildcard_matches(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let mut dp = vec![false; text.len() + 1];
    dp[0] = true;
    for &token in pattern {
        if token == b'*' {
            for index in 1..=text.len() {
                dp[index] = dp[index] || dp[index - 1];
            }
            continue;
        }
        for index in (1..=text.len()).rev() {
            dp[index] = dp[index - 1] && (token == b'?' || token == text[index - 1]);
        }
        dp[0] = false;
    }
    dp[text.len()]
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

fn required_path_input<'a>(tool_name: &str, input: &'a Value) -> Result<&'a str, RuntimeError> {
    required_input_string_alias(tool_name, input, "path", "filePath")
}

fn optional_path_input<'a>(
    tool_name: &str,
    input: &'a Value,
) -> Result<Option<&'a str>, RuntimeError> {
    optional_string_alias_input(tool_name, input, "path", "filePath")
}

fn required_input_string_alias<'a>(
    tool_name: &str,
    input: &'a Value,
    field: &str,
    alias: &str,
) -> Result<&'a str, RuntimeError> {
    if let Some(value) = input.get(field) {
        return value.as_str().ok_or_else(|| {
            RuntimeError::Provider(format!("tool {tool_name} input.{field} must be a string"))
        });
    }
    if let Some(value) = input.get(alias) {
        return value.as_str().ok_or_else(|| {
            RuntimeError::Provider(format!("tool {tool_name} input.{alias} must be a string"))
        });
    }
    Err(RuntimeError::Provider(format!(
        "tool {tool_name} input.{field} must be a string"
    )))
}

fn required_labeled_string_alias<'a>(
    tool_name: &str,
    input: &'a Value,
    label: &str,
    field: &str,
    alias: &str,
) -> Result<&'a str, RuntimeError> {
    if let Some(value) = input.get(field) {
        return value.as_str().ok_or_else(|| {
            RuntimeError::Provider(format!("tool {tool_name} {label}.{field} must be a string"))
        });
    }
    if let Some(value) = input.get(alias) {
        return value.as_str().ok_or_else(|| {
            RuntimeError::Provider(format!("tool {tool_name} {label}.{alias} must be a string"))
        });
    }
    Err(RuntimeError::Provider(format!(
        "tool {tool_name} {label}.{field} must be a string"
    )))
}

fn optional_string_alias_input<'a>(
    tool_name: &str,
    input: &'a Value,
    field: &str,
    alias: &str,
) -> Result<Option<&'a str>, RuntimeError> {
    if let Some(value) = input.get(field) {
        return value.as_str().map(Some).ok_or_else(|| {
            RuntimeError::Provider(format!("tool {tool_name} input.{field} must be a string"))
        });
    }
    if let Some(value) = input.get(alias) {
        return value.as_str().map(Some).ok_or_else(|| {
            RuntimeError::Provider(format!("tool {tool_name} input.{alias} must be a string"))
        });
    }
    Ok(None)
}

fn optional_bool_alias_input(
    tool_name: &str,
    input: &Value,
    field: &str,
    alias: &str,
) -> Result<Option<bool>, RuntimeError> {
    if input.get(field).is_some() {
        return optional_bool_input(tool_name, input, field);
    }
    let Some(value) = input.get(alias) else {
        return Ok(None);
    };
    value.as_bool().map(Some).ok_or_else(|| {
        RuntimeError::Provider(format!("tool {tool_name} input.{alias} must be a boolean"))
    })
}

fn optional_labeled_bool_alias_input(
    tool_name: &str,
    input: &Value,
    label: &str,
    field: &str,
    alias: &str,
) -> Result<Option<bool>, RuntimeError> {
    if input.get(field).is_some() {
        return optional_labeled_bool_input(tool_name, input, label, field);
    }
    let Some(value) = input.get(alias) else {
        return Ok(None);
    };
    value.as_bool().map(Some).ok_or_else(|| {
        RuntimeError::Provider(format!(
            "tool {tool_name} {label}.{alias} must be a boolean"
        ))
    })
}

fn parse_alias_usize(
    tool_name: &str,
    input: &Value,
    alias: &str,
    configured_max: usize,
    integer_qualifier: &str,
    reject_zero: bool,
) -> Result<Option<usize>, RuntimeError> {
    let Some(value) = input.get(alias) else {
        return Ok(None);
    };
    let Some(number) = value.as_u64() else {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.{alias} must be {integer_qualifier} integer"
        )));
    };
    if reject_zero && number == 0 {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.{alias} must be greater than 0"
        )));
    }
    usize::try_from(number)
        .map(|value| Some(value.min(configured_max)))
        .map_err(|_| RuntimeError::Provider(format!("tool {tool_name} input.{alias} is too large")))
}

fn optional_bounded_usize_alias_input(
    tool_name: &str,
    input: &Value,
    field: &str,
    alias: &str,
    configured_max: usize,
) -> Result<Option<usize>, RuntimeError> {
    if input.get(field).is_some() {
        return optional_bounded_usize_input(tool_name, input, field, configured_max);
    }
    parse_alias_usize(tool_name, input, alias, configured_max, "a positive", true)
}

fn optional_bounded_zero_or_positive_usize_alias_input(
    tool_name: &str,
    input: &Value,
    field: &str,
    alias: &str,
    configured_max: usize,
) -> Result<Option<usize>, RuntimeError> {
    if input.get(field).is_some() {
        return optional_bounded_zero_or_positive_usize_input(
            tool_name,
            input,
            field,
            configured_max,
        );
    }
    parse_alias_usize(
        tool_name,
        input,
        alias,
        configured_max,
        "a non-negative",
        false,
    )
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ReadSnapshot {
    pub(super) modified: SystemTime,
    fingerprint: FileFingerprint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileFingerprint {
    len: u64,
    hash: u64,
}

pub(super) fn read_snapshot(
    name: &str,
    label: &str,
    path: &Path,
) -> Result<ReadSnapshot, RuntimeError> {
    let modified = file_modified_time(name, label, path)?;
    let bytes = fs::read(path).map_err(|error| {
        RuntimeError::Provider(format!(
            "tool {name} read content fingerprint for {label}: {error}"
        ))
    })?;
    Ok(ReadSnapshot {
        modified,
        fingerprint: file_fingerprint(&bytes),
    })
}

fn file_fingerprint(bytes: &[u8]) -> FileFingerprint {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    FileFingerprint {
        len: bytes.len() as u64,
        hash,
    }
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
                    return Ok(Self::Auto);
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

fn code_file_requires_complete_line_anchor(path: &str) -> bool {
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    matches!(
        extension,
        "c" | "cc"
            | "cpp"
            | "cs"
            | "css"
            | "go"
            | "h"
            | "hpp"
            | "java"
            | "js"
            | "jsx"
            | "kt"
            | "lua"
            | "php"
            | "py"
            | "rb"
            | "rs"
            | "scala"
            | "sh"
            | "swift"
            | "ts"
            | "tsx"
            | "vue"
    )
}

fn single_line_code_anchor_is_incomplete(
    content: &str,
    edit_match: EditMatch,
    old_string: &str,
) -> bool {
    if old_string.contains('\n') || old_string.trim().is_empty() {
        return false;
    }
    let line_start = content[..edit_match.start]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let line_end = content[edit_match.end..]
        .find('\n')
        .map(|index| edit_match.end + index)
        .unwrap_or(content.len());
    let line = content[line_start..line_end].trim_end_matches('\r');
    line.trim() != old_string.trim()
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

fn find_line_block_edit_matches(
    content: &str,
    search: &str,
    mut matches: Vec<EditMatch>,
    block_eq: impl Fn(&str, &str) -> bool,
) -> Vec<EditMatch> {
    let content_lines = split_lines_with_offsets(content);
    let search_lines = normalized_search_lines(search);
    if search_lines.is_empty() || search_lines.len() > content_lines.len() {
        return dedup_edit_matches(matches);
    }
    for start_line in 0..=content_lines.len() - search_lines.len() {
        let end_line = start_line + search_lines.len() - 1;
        let block = &content[content_lines[start_line].start..content_lines[end_line].end];
        if block_eq(block, search) {
            matches.push(EditMatch {
                start: content_lines[start_line].start,
                end: content_lines[end_line].end,
            });
        }
    }
    dedup_edit_matches(matches)
}

fn find_escape_normalized_edit_matches(content: &str, old_string: &str) -> Vec<EditMatch> {
    let unescaped_search = unescape_edit_string(old_string);
    let matches = content
        .match_indices(&unescaped_search)
        .map(|(start, value)| EditMatch {
            start,
            end: start + value.len(),
        })
        .collect::<Vec<_>>();
    find_line_block_edit_matches(content, &unescaped_search, matches, |block, search| {
        unescape_edit_string(block) == search
    })
}

fn find_trimmed_boundary_edit_matches(content: &str, old_string: &str) -> Vec<EditMatch> {
    let trimmed_search = old_string.trim();
    if trimmed_search == old_string || trimmed_search.is_empty() {
        return Vec::new();
    }

    let matches = content
        .match_indices(trimmed_search)
        .map(|(start, value)| EditMatch {
            start,
            end: start + value.len(),
        })
        .collect::<Vec<_>>();
    find_line_block_edit_matches(content, trimmed_search, matches, |block, search| {
        block.trim() == search
    })
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
}

pub(super) struct FileEditOptions<'a> {
    pub(super) base_dir: &'a Path,
    pub(super) max_bytes: usize,
    pub(super) max_changed_lines: Option<usize>,
}

pub(super) fn call_file_write_tool(
    name: &str,
    input: &Value,
    options: FileWriteOptions<'_>,
) -> Result<Value, RuntimeError> {
    let input_path = required_input_string(name, input, "path")?;
    let content = required_input_string(name, input, "content")?;
    let content_bytes = content.as_bytes();
    if content_bytes.len() > options.max_bytes {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.content exceeds max_bytes={}",
            options.max_bytes
        )));
    }
    let base = canonicalize_tool_path(name, "base_dir", options.base_dir)?;
    let candidate = if Path::new(input_path).is_absolute() {
        PathBuf::from(input_path)
    } else {
        base.join(input_path)
    };
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

fn build_post_edit_snippets(path: &str, content: &str, changed_lines: &[usize]) -> Value {
    let total_lines = content.lines().count().max(1);
    let snippets = merge_line_ranges(changed_lines, total_lines, 3)
        .into_iter()
        .take(8)
        .map(|(start_line, end_line)| {
            let selected = select_line_range(content, Some(start_line), Some(end_line));
            json!({
                "path": path,
                "start_line": start_line,
                "end_line": end_line,
                "content_format": "line_numbered",
                "content": numbered_content(&selected, start_line)
            })
        })
        .collect::<Vec<_>>();
    Value::Array(snippets)
}

fn parse_file_edit_operations<'a>(
    name: &str,
    input: &'a Value,
) -> Result<Vec<FileEditOperation<'a>>, RuntimeError> {
    let global_replace_all =
        optional_bool_alias_input(name, input, "replace_all", "replaceAll")?.unwrap_or(false);
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
            let old_string =
                required_labeled_string_alias(name, edit, &label, "old_string", "oldString")?;
            let new_string =
                required_labeled_string_alias(name, edit, &label, "new_string", "newString")?;
            let replace_all =
                optional_labeled_bool_alias_input(name, edit, &label, "replace_all", "replaceAll")?
                    .unwrap_or(global_replace_all);
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

    let old_string = required_input_string_alias(name, input, "old_string", "oldString")?;
    let new_string = required_input_string_alias(name, input, "new_string", "newString")?;
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

fn edit_anchor_line(content: &str, old_string: &str) -> Option<usize> {
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

struct EditDiagnostic {
    label: String,
    path: String,
    field: &'static str,
    message: String,
    match_strategy: Option<&'static str>,
    effective_match_strategy: Option<&'static str>,
    match_count: Option<usize>,
    line: Option<usize>,
}

impl EditDiagnostic {
    fn into_json(self, source: &str) -> Value {
        json!({
            "source": source,
            "severity": "error",
            "message": self.message,
            "operation_label": self.label,
            "path": self.path,
            "kind": "edit",
            "field": self.field,
            "match_strategy": self.match_strategy,
            "effective_match_strategy": self.effective_match_strategy,
            "match_count": self.match_count,
            "line": self.line,
            "line_source": self.line.map(|_| "old_string_anchor")
        })
    }
}

fn provider_error_message(error: &RuntimeError) -> String {
    match error {
        RuntimeError::Provider(message) => message.clone(),
        other => other.to_string(),
    }
}

fn edit_input_error_field(message: &str) -> &'static str {
    if message.contains("old_string") || message.contains("oldString") {
        "old_string"
    } else if message.contains("new_string") || message.contains("newString") {
        "new_string"
    } else {
        "input"
    }
}

fn is_structured_edit_input_field_error(message: &str) -> bool {
    message.contains("old_string must be a string")
        || message.contains("oldString must be a string")
        || message.contains("new_string must be a string")
        || message.contains("newString must be a string")
}

fn file_edit_failure_output(
    name: &str,
    base: &Path,
    path: &Path,
    input_path: &str,
    diff: &str,
    diagnostic: EditDiagnostic,
) -> Value {
    let (diff_content, diff_truncated, diff_bytes) =
        bytes_to_limited_text(diff.as_bytes(), 64 * 1024);
    json!({
        "repo": base.display().to_string(),
        "path": path.display().to_string(),
        "success": false,
        "checked": true,
        "applied": false,
        "files": [{
            "path": input_path,
            "kind": "existing"
        }],
        "file_count": 1,
        "diagnostics": [diagnostic.into_json(name)],
        "bytes": diff_bytes,
        "diff": diff_content,
        "diff_truncated": diff_truncated,
        "artifacts": [{
            "id": format!("file-edit:{}:failed", path.display()),
            "kind": "file_edit",
            "title": "file edit failed validation",
            "uri": path.display().to_string(),
            "content": diff_content.clone(),
            "metadata": {
                "provider": "file_edit",
                "path": path.display().to_string(),
                "repo": base.display().to_string(),
                "success": false,
                "checked": true,
                "applied": false,
                "diff_bytes": diff_bytes,
                "diff_truncated": diff_truncated
            }
        }]
    })
}

pub(super) fn call_file_edit_tool(
    name: &str,
    input: &Value,
    options: FileEditOptions<'_>,
) -> Result<Value, RuntimeError> {
    let input_path = required_path_input(name, input)?;
    let dry_run = optional_bool_input(name, input, "dry_run")?.unwrap_or(false);
    let max_changed_lines = effective_max_changed_lines(name, input, options.max_changed_lines)?;

    let base = canonicalize_tool_path(name, "base_dir", options.base_dir)?;
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
    let operations = match parse_file_edit_operations(name, input) {
        Ok(operations) => operations,
        Err(error) => {
            let message = provider_error_message(&error);
            if !is_structured_edit_input_field_error(&message) {
                return Err(error);
            }
            return Ok(file_edit_failure_output(
                name,
                &base,
                &path,
                input_path,
                "",
                EditDiagnostic {
                    label: "input".to_string(),
                    path: input_path.to_string(),
                    field: edit_input_error_field(&message),
                    message,
                    match_strategy: None,
                    effective_match_strategy: None,
                    match_count: None,
                    line: None,
                },
            ));
        }
    };

    let content = fs::read_to_string(&path)
        .map_err(|error| RuntimeError::Provider(format!("tool {name} read file: {error}")))?;
    let mut updated = content.clone();
    let mut total_replacements = 0usize;
    let mut diff = String::new();
    let mut strategies = Vec::new();
    let mut changed_start_lines = Vec::new();
    for operation in &operations {
        let (effective_match_strategy, matches) =
            find_edit_matches(&updated, operation.old_string, operation.match_strategy);
        if matches.is_empty() {
            return Ok(file_edit_failure_output(
                name,
                &base,
                &path,
                input_path,
                &diff,
                EditDiagnostic {
                    label: operation.label.clone(),
                    path: input_path.to_string(),
                    field: "old_string",
                    message: format!(
                        "{}.old_string was not found with match_strategy={}",
                        operation.label,
                        operation.match_strategy.as_str()
                    ),
                    match_strategy: Some(operation.match_strategy.as_str()),
                    effective_match_strategy: None,
                    match_count: Some(0),
                    line: edit_anchor_line(&updated, operation.old_string),
                },
            ));
        }
        if matches.len() > 1 && !operation.replace_all {
            return Ok(file_edit_failure_output(
                name,
                &base,
                &path,
                input_path,
                &diff,
                EditDiagnostic {
                    label: operation.label.clone(),
                    path: input_path.to_string(),
                    field: "old_string",
                    message: format!(
                        "{}.old_string matched {} times with match_strategy={}; set replaceAll=true only when all matches should change",
                        operation.label,
                        matches.len(),
                        effective_match_strategy.as_str()
                    ),
                    match_strategy: Some(operation.match_strategy.as_str()),
                    effective_match_strategy: Some(effective_match_strategy.as_str()),
                    match_count: Some(matches.len()),
                    line: None,
                },
            ));
        }
        let selected_matches = selected_edit_matches(&matches, operation.replace_all);
        if code_file_requires_complete_line_anchor(input_path)
            && selected_matches.iter().any(|edit_match| {
                single_line_code_anchor_is_incomplete(&updated, *edit_match, operation.old_string)
            })
        {
            return Ok(file_edit_failure_output(
                name,
                &base,
                &path,
                input_path,
                &diff,
                EditDiagnostic {
                    label: operation.label.clone(),
                    path: input_path.to_string(),
                    field: "old_string",
                    message: format!(
                        "{}.old_string must include a complete source line or multi-line context for code files",
                        operation.label
                    ),
                    match_strategy: Some(operation.match_strategy.as_str()),
                    effective_match_strategy: Some(effective_match_strategy.as_str()),
                    match_count: Some(matches.len()),
                    line: edit_anchor_line(&updated, operation.old_string),
                },
            ));
        }
        diff.push_str(&edit_unified_diff(
            input_path,
            &updated,
            &selected_matches,
            operation.new_string,
        ));
        changed_start_lines.extend(
            selected_matches
                .iter()
                .map(|edit_match| line_number_at(&updated, edit_match.start)),
        );
        total_replacements += selected_matches.len();
        strategies.push(effective_match_strategy.as_str());
        updated = apply_selected_edit_matches(&updated, &selected_matches, operation.new_string);
    }

    let updated_bytes = updated.as_bytes();
    if updated_bytes.len() > options.max_bytes {
        return Ok(file_edit_failure_output(
            name,
            &base,
            &path,
            input_path,
            &diff,
            EditDiagnostic {
                label: "input".to_string(),
                path: input_path.to_string(),
                field: "new_string",
                message: format!("edited content exceeds max_bytes={}", options.max_bytes),
                match_strategy: None,
                effective_match_strategy: None,
                match_count: None,
                line: None,
            },
        ));
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
        "post_edit_snippets": build_post_edit_snippets(input_path, &updated, &changed_start_lines),
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
