use super::file_search_render::{render_search_matches, stable_pattern_id};

use super::*;
use air_tools_core::text::{bytes_to_limited_text, select_line_range};

const DEFAULT_UNSCOPED_READ_LINE_LIMIT: usize = 200;
const DEFAULT_OPEN_ENDED_RANGE_LINE_LIMIT: usize = DEFAULT_UNSCOPED_READ_LINE_LIMIT;

fn is_unscoped_read_request(
    contains: Option<&str>,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> bool {
    contains.is_none() && start_line.is_none() && end_line.is_none()
}

pub(super) fn call_file_read_tool(
    name: &str,
    input: &Value,
    base_dir: &Path,
    max_bytes: usize,
) -> Result<Value, RuntimeError> {
    let input_path = required_path_input(name, input)?;
    let base = canonicalize_tool_path(name, "base_dir", base_dir)?;
    let resolved = resolve_existing_input_path_in_base(name, "input.path", &base, input_path)?;
    let path_rebased_from = resolved.rebased_from.clone();
    let path = resolved.path;
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
            start.map(|start| {
                start
                    .saturating_add(limit.unwrap_or(DEFAULT_OPEN_ENDED_RANGE_LINE_LIMIT))
                    .saturating_sub(1)
            })
        });
        (start, end)
    } else if let Some(offset) = offset {
        let start = offset.saturating_add(1);
        let end = Some(
            start
                .saturating_add(limit.unwrap_or(DEFAULT_OPEN_ENDED_RANGE_LINE_LIMIT))
                .saturating_sub(1),
        );
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
    let requested_unscoped_read = is_unscoped_read_request(contains, start_line, end_line);
    if requested_unscoped_read {
        return Err(RuntimeError::Provider(format!(
            "tool {name} requires a bounded selector: use offset+limit, start_line/end_line, lines, or contains+context_lines"
        )));
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
    let effective_end_line = effective_end_line.map(|line| line.min(total_lines));
    let selected = select_line_range(&full_content, effective_start_line, effective_end_line);
    let selected_max_bytes = effective_max_bytes;
    let (limited_selected, truncated, _source_window_bytes) =
        bytes_to_limited_text(selected.as_bytes(), selected_max_bytes);
    let content = if line_numbers {
        numbered_content(&limited_selected, effective_start_line.unwrap_or(1))
    } else {
        limited_selected
    };
    let bytes = content.len();
    let full_output_path = if truncated {
        let full_display_content = if line_numbers {
            numbered_content(&selected, effective_start_line.unwrap_or(1))
        } else {
            selected.clone()
        };
        maybe_save_full_output(
            name,
            &base,
            "file-read",
            full_display_content.as_bytes(),
            true,
        )?
    } else {
        None
    };
    let unscoped_read = requested_unscoped_read;
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
            "line_numbers_defaulted": !explicit_line_numbers && line_numbers,
            "selected_max_bytes": selected_max_bytes,
            "path_rebased_from": path_rebased_from
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
        "selected_max_bytes": selected_max_bytes,
        "truncated": truncated,
        "full_output_path": full_output_path,
        "truncation_hint": truncation_hint,
        "unscoped_read": unscoped_read,
        "line_numbers": line_numbers,
        "line_numbers_defaulted": !explicit_line_numbers && line_numbers,
        "path_rebased_from": path_rebased_from,
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
                "Output was truncated from {} bytes.{saved} Use file.search, contains+context_lines, or exact line ranges to inspect a narrow span instead of repeating an unbounded request.",
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
    let path = resolve_existing_input_path_in_base(name, "input.path", &base, &input_path)?.path;
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
    let rendered = render_search_matches(&result.matches, Some(&base));
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
            "base_path": base.display().to_string(),
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
        "base_path": base.display().to_string(),
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
    format!("{scope} broaden the pattern/path, inspect the repo file list, or read a known likely file instead of repeating speculative searches.")
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
        parts.push("Use file.search with a narrower pattern or exact line ranges to inspect specific sections.".to_string());
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
        let before = collect_context_lines(
            &lines,
            before_start..line_number,
            effective_max_line_chars,
            relative_path,
            result,
        );
        let after_end = (line_number + context_lines).min(total_lines);
        let after = collect_context_lines(
            &lines,
            (line_number + 1)..=after_end,
            effective_max_line_chars,
            relative_path,
            result,
        );
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

fn collect_context_lines(
    lines: &[&str],
    line_numbers: impl Iterator<Item = usize>,
    max_line_chars: usize,
    relative_path: Option<&str>,
    result: &mut FileSearchResult,
) -> Vec<Value> {
    line_numbers
        .filter_map(|number| line_json_for_path(lines, number, max_line_chars, relative_path))
        .inspect(|line| {
            result.any_line_truncated |= line
                .get("line_truncated")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        })
        .collect()
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
        Some(".air" | ".git" | "target" | "node_modules" | "dist" | "build" | "__pycache__")
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

pub(super) fn required_path_input<'a>(
    tool_name: &str,
    input: &'a Value,
) -> Result<&'a str, RuntimeError> {
    let fields = ["file", "path", "filePath"];
    let present = fields
        .iter()
        .copied()
        .filter(|field| input.get(*field).is_some())
        .collect::<Vec<_>>();
    if present.len() > 1 {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} must provide only one of input.file, input.path, or input.filePath"
        )));
    }
    let Some(field) = present.first() else {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.path must be a string"
        )));
    };
    input.get(*field).and_then(Value::as_str).ok_or_else(|| {
        RuntimeError::Provider(format!("tool {tool_name} input.{field} must be a string"))
    })
}

#[derive(Debug, Clone)]
pub(super) struct ResolvedToolPath {
    pub(super) path: PathBuf,
    pub(super) rebased_from: Option<String>,
}

pub(super) fn resolve_existing_input_path_in_base(
    tool_name: &str,
    label: &str,
    base: &Path,
    input_path: &str,
) -> Result<ResolvedToolPath, RuntimeError> {
    let input = Path::new(input_path);
    let candidate = if input.is_absolute() {
        input.to_path_buf()
    } else {
        base.join(input)
    };
    match canonicalize_tool_path(tool_name, label, &candidate) {
        Ok(path) if path.starts_with(base) => Ok(ResolvedToolPath {
            path,
            rebased_from: None,
        }),
        Ok(_) | Err(_) if input.is_absolute() => {
            if let Some(path) =
                rebase_existing_absolute_path_by_suffix(tool_name, label, base, input)?
            {
                return Ok(ResolvedToolPath {
                    path,
                    rebased_from: Some(input_path.to_string()),
                });
            }
            Err(outside_base_error(tool_name))
        }
        Ok(_) => Err(outside_base_error(tool_name)),
        Err(error) => Err(error),
    }
}

pub(super) fn resolve_writable_input_path_in_base(
    tool_name: &str,
    label: &str,
    base: &Path,
    input_path: &str,
    create_dirs: bool,
) -> Result<ResolvedToolPath, RuntimeError> {
    let input = Path::new(input_path);
    if input.is_absolute() {
        if let Some(path) = rebase_writable_absolute_path_by_suffix(tool_name, label, base, input)?
        {
            return Ok(ResolvedToolPath {
                path,
                rebased_from: Some(input_path.to_string()),
            });
        }
    }
    let candidate = if input.is_absolute() {
        input.to_path_buf()
    } else {
        base.join(input)
    };
    let path = resolve_writable_candidate(tool_name, label, base, &candidate, create_dirs)?;
    Ok(ResolvedToolPath {
        path,
        rebased_from: None,
    })
}

fn rebase_existing_absolute_path_by_suffix(
    tool_name: &str,
    label: &str,
    base: &Path,
    input: &Path,
) -> Result<Option<PathBuf>, RuntimeError> {
    for suffix in absolute_path_suffixes(input, 3) {
        let candidate = base.join(&suffix);
        if !candidate.exists() {
            continue;
        }
        let path = canonicalize_tool_path(tool_name, label, &candidate)?;
        if path.starts_with(base) {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn rebase_writable_absolute_path_by_suffix(
    tool_name: &str,
    label: &str,
    base: &Path,
    input: &Path,
) -> Result<Option<PathBuf>, RuntimeError> {
    for suffix in absolute_path_suffixes(input, 3) {
        let candidate = base.join(&suffix);
        if let Ok(path) = resolve_writable_candidate(tool_name, label, base, &candidate, false) {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn absolute_path_suffixes(path: &Path, min_components: usize) -> Vec<PathBuf> {
    let components = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_os_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if components.len() < min_components {
        return Vec::new();
    }
    (0..=components.len() - min_components)
        .map(|start| components[start..].iter().collect())
        .collect()
}

fn resolve_writable_candidate(
    tool_name: &str,
    label: &str,
    base: &Path,
    candidate: &Path,
    create_dirs: bool,
) -> Result<PathBuf, RuntimeError> {
    let parent = candidate.parent().ok_or_else(|| {
        RuntimeError::Provider(format!(
            "tool {tool_name} {label} must have a parent directory"
        ))
    })?;
    if create_dirs && can_create_parent_inside_base(tool_name, label, candidate, base)? {
        fs::create_dir_all(parent).map_err(|error| {
            RuntimeError::Provider(format!(
                "tool {tool_name} create parent directories: {error}"
            ))
        })?;
    }
    let parent = canonicalize_tool_path(tool_name, &format!("{label} parent"), parent)?;
    if !parent.starts_with(base) {
        return Err(outside_base_error(tool_name));
    }
    let file_name = candidate.file_name().ok_or_else(|| {
        RuntimeError::Provider(format!("tool {tool_name} {label} must include a file name"))
    })?;
    Ok(parent.join(file_name))
}

fn can_create_parent_inside_base(
    tool_name: &str,
    label: &str,
    path: &Path,
    base: &Path,
) -> Result<bool, RuntimeError> {
    if has_parent_dir_component(path) {
        return Ok(false);
    }
    let Some(mut ancestor) = path.parent() else {
        return Ok(false);
    };
    while !ancestor.exists() {
        let Some(parent) = ancestor.parent() else {
            return Ok(false);
        };
        ancestor = parent;
    }
    let ancestor = canonicalize_tool_path(tool_name, &format!("{label} ancestor"), ancestor)?;
    Ok(ancestor.starts_with(base))
}

fn has_parent_dir_component(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
}

fn outside_base_error(tool_name: &str) -> RuntimeError {
    RuntimeError::Provider(format!(
        "tool {tool_name} input.path is outside configured base_dir. Use a workspace-relative path or an exact path returned by discovery, search, or span-inspection tools."
    ))
}

fn optional_path_input<'a>(
    tool_name: &str,
    input: &'a Value,
) -> Result<Option<&'a str>, RuntimeError> {
    optional_string_alias_input(tool_name, input, "path", "filePath")
}

pub(super) fn required_input_string_alias<'a>(
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

pub(super) fn required_labeled_string_alias<'a>(
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

pub(super) fn optional_string_alias_input<'a>(
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

pub(super) fn optional_bool_alias_input(
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

pub(super) fn optional_labeled_bool_alias_input(
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

pub(super) fn numbered_content(content: &str, first_line: usize) -> String {
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

pub(super) struct FileWriteOptions<'a> {
    pub(super) base_dir: &'a Path,
    pub(super) max_bytes: usize,
    pub(super) create_dirs: bool,
    pub(super) allow_overwrite: bool,
}

pub(super) fn call_file_write_tool(
    name: &str,
    input: &Value,
    options: FileWriteOptions<'_>,
) -> Result<Value, RuntimeError> {
    let input_path = required_path_input(name, input)?;
    let content = required_input_string(name, input, "content")?;
    let content_bytes = content.as_bytes();
    if content_bytes.len() > options.max_bytes {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.content exceeds max_bytes={}",
            options.max_bytes
        )));
    }
    let base = canonicalize_tool_path(name, "base_dir", options.base_dir)?;
    let resolved = resolve_writable_input_path_in_base(
        name,
        "input.path",
        &base,
        input_path,
        options.create_dirs,
    )?;
    let path_rebased_from = resolved.rebased_from.clone();
    let mut path = resolved.path;
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
        "workspace_changed": true,
        "path_rebased_from": path_rebased_from,
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
                "overwritten": existed,
                "workspace_changed": true,
                "path_rebased_from": path_rebased_from
            }
        }]
    }))
}
