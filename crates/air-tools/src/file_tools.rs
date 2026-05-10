use super::*;

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
    let start_line = optional_positive_usize_input(name, input, "start_line")?;
    let end_line = optional_positive_usize_input(name, input, "end_line")?;
    let contains = optional_string_input(name, input, "contains")?;
    let context_lines = optional_positive_usize_input(name, input, "context_lines")?.unwrap_or(0);
    let occurrence = optional_positive_usize_input(name, input, "occurrence")?.unwrap_or(1);
    let line_numbers = optional_bool_input(name, input, "line_numbers")?.unwrap_or(false);
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
    let (content, truncated, bytes) = bytes_to_limited_text(selected.as_bytes(), max_bytes);
    let numbered_content = if line_numbers {
        Some(numbered_content(
            &content,
            effective_start_line.unwrap_or(1),
        ))
    } else {
        None
    };
    Ok(json!({
        "path": path.display().to_string(),
        "content": content.clone(),
        "numbered_content": numbered_content,
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
        "line_numbers": line_numbers,
        "artifacts": [{
            "id": format!("file:{}", path.display()),
            "kind": "file_span",
            "title": path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_else(|| path.to_str().unwrap_or("file")),
            "uri": path.display().to_string(),
            "content": content.clone(),
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
                "line_numbers": line_numbers
            }
        }],
    }))
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

    let mut files = Vec::with_capacity(entries.len());
    let mut artifacts = Vec::new();
    let mut total_bytes = 0usize;
    let mut any_truncated = false;
    for entry in entries {
        let output = call_file_read_tool(name, &entry, base_dir, max_bytes)?;
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
) -> Result<Value, RuntimeError> {
    let input_path = required_input_string(name, input, "path")?;
    let pattern = required_input_string(name, input, "pattern")?;
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
    let body = fs::read(&path)
        .map_err(|error| RuntimeError::Provider(format!("tool {name} read file: {error}")))?;
    if is_likely_binary(&body) {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.path appears to be binary; file_search only supports UTF-8 text"
        )));
    }
    let content = std::str::from_utf8(&body).map_err(|_| {
        RuntimeError::Provider(format!(
            "tool {name} input.path is not valid UTF-8; file_search only supports UTF-8 text"
        ))
    })?;
    let context_lines = optional_bounded_zero_or_positive_usize_input(
        name,
        input,
        "context_lines",
        max_context_lines,
    )?
    .unwrap_or(0);
    let lines = content.lines().collect::<Vec<_>>();
    let total_lines = lines.len();
    let mut total_match_count = 0usize;
    let mut matches = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !regex.is_match(line) {
            continue;
        }
        total_match_count += 1;
        if matches.len() >= max_matches {
            continue;
        }
        let line_number = index + 1;
        let before_start = line_number.saturating_sub(context_lines).max(1);
        let before = (before_start..line_number)
            .filter_map(|number| line_json(&lines, number))
            .collect::<Vec<_>>();
        let after_end = (line_number + context_lines).min(total_lines);
        let after = ((line_number + 1)..=after_end)
            .filter_map(|number| line_json(&lines, number))
            .collect::<Vec<_>>();
        matches.push(json!({
            "line_number": line_number,
            "line": line,
            "before": before,
            "after": after,
        }));
    }

    let mut rendered = String::new();
    for item in &matches {
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
    let match_truncated = total_match_count > matches.len();
    Ok(json!({
        "path": path.display().to_string(),
        "pattern": pattern,
        "matches": matches,
        "match_count": total_match_count,
        "returned_match_count": matches.len(),
        "total_lines": total_lines,
        "context_lines": context_lines,
        "truncated": match_truncated || content_truncated,
        "bytes": bytes,
        "artifacts": [{
            "id": format!("file-search:{}:{}", path.display(), stable_pattern_id(pattern)),
            "kind": "file_search",
            "title": format!("{} matches in {}", total_match_count, path.file_name().and_then(|name| name.to_str()).unwrap_or("file")),
            "uri": path.display().to_string(),
            "content": artifact_content,
            "metadata": {
                "provider": "file_search",
                "path": path.display().to_string(),
                "pattern": pattern,
                "match_count": total_match_count,
                "returned_match_count": matches.len(),
                "total_lines": total_lines,
                "context_lines": context_lines,
                "max_matches": max_matches,
                "truncated": match_truncated || content_truncated
            }
        }]
    }))
}

fn line_json(lines: &[&str], line_number: usize) -> Option<Value> {
    lines.get(line_number.checked_sub(1)?).map(|line| {
        json!({
            "line_number": line_number,
            "line": line,
        })
    })
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
    format!("{number}: {line}\n")
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditMatchStrategy {
    Auto,
    Exact,
    LineTrimmed,
    WhitespaceNormalized,
    IndentationFlexible,
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
            "exact" => Ok(Self::Exact),
            "line_trimmed" => Ok(Self::LineTrimmed),
            "whitespace_normalized" => Ok(Self::WhitespaceNormalized),
            "indentation_flexible" => Ok(Self::IndentationFlexible),
            _ => Err(RuntimeError::Provider(format!(
                "tool {tool_name} {label}.match_strategy must be one of auto, exact, line_trimmed, whitespace_normalized, indentation_flexible"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Exact => "exact",
            Self::LineTrimmed => "line_trimmed",
            Self::WhitespaceNormalized => "whitespace_normalized",
            Self::IndentationFlexible => "indentation_flexible",
        }
    }
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
        for candidate in [
            EditMatchStrategy::Exact,
            EditMatchStrategy::LineTrimmed,
            EditMatchStrategy::IndentationFlexible,
            EditMatchStrategy::WhitespaceNormalized,
        ] {
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
        EditMatchStrategy::Exact => content
            .match_indices(old_string)
            .map(|(start, value)| EditMatch {
                start,
                end: start + value.len(),
            })
            .collect(),
        EditMatchStrategy::LineTrimmed => {
            find_line_based_edit_matches(content, old_string, |line| line.trim().to_string())
        }
        EditMatchStrategy::WhitespaceNormalized => {
            find_line_based_edit_matches(content, old_string, normalize_whitespace)
        }
        EditMatchStrategy::IndentationFlexible => {
            find_line_based_edit_matches(content, old_string, strip_common_indentation)
        }
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
    pub(super) require_read: bool,
    pub(super) allow_new_files: bool,
    pub(super) allow_overwrite: bool,
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

pub(super) fn call_file_ops_tool(
    name: &str,
    input: &Value,
    options: FileOpsOptions<'_>,
) -> Result<Value, RuntimeError> {
    let dry_run = optional_bool_input(name, input, "dry_run")?.unwrap_or(false);
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
    let mut pending = BTreeMap::<PathBuf, FileOpsPendingFile>::new();
    let mut diff = String::new();
    let mut match_strategies = Vec::new();

    for (index, operation) in operations.iter().enumerate() {
        let label = format!("input.operations[{index}]");
        let Some(object) = operation.as_object() else {
            return Err(RuntimeError::Provider(format!(
                "tool {name} {label} must be an object"
            )));
        };
        let kind = required_labeled_string_input(name, operation, &label, "kind")?;
        let input_path = required_labeled_string_input(name, operation, &label, "path")?;
        validate_git_pathspec(name, input_path)?;
        let candidate = base.join(input_path);

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
                let new_string =
                    required_labeled_string_input(name, operation, &label, "new_string")?;
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
                    return Ok(file_ops_failure_output(
                        name,
                        &base,
                        operations.len(),
                        &pending,
                        &diff,
                        format!(
                            "{label}.old_string was not found with match_strategy={}",
                            match_strategy.as_str()
                        ),
                    ));
                }
                if matches.len() > 1 && !replace_all {
                    return Ok(file_ops_failure_output(
                        name,
                        &base,
                        operations.len(),
                        &pending,
                        &diff,
                        format!(
                            "{label}.old_string matched {} times with match_strategy={}; set replace_all=true only when all matches should change",
                        matches.len(),
                        effective_match_strategy.as_str()
                        ),
                    ));
                }
                let selected = selected_edit_matches(&matches, replace_all);
                match_strategies.push(effective_match_strategy.as_str());
                diff.push_str(&edit_unified_diff(
                    input_path, &current, &selected, new_string,
                ));
                let updated = apply_selected_edit_matches(&current, &selected, new_string);
                if updated.len() > options.max_bytes {
                    return Ok(file_ops_failure_output(
                        name,
                        &base,
                        operations.len(),
                        &pending,
                        &diff,
                        format!(
                            "{label} edited content exceeds max_bytes={}",
                            options.max_bytes
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
                        format!("{label}.content exceeds max_bytes={}", options.max_bytes),
                    ));
                }
                let (path, existed, old_content) =
                    resolve_file_ops_write_path(name, &label, &base, input_path)?;
                if existed {
                    if !options.allow_overwrite {
                        return Err(RuntimeError::Provider(format!(
                            "tool {name} {label}.path already exists and allow_overwrite is false"
                        )));
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
                    return Err(RuntimeError::Provider(format!(
                        "tool {name} {label}.path does not exist and allow_new_files is false"
                    )));
                }
                diff.push_str(&write_unified_diff(
                    input_path,
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
                    "tool {name} {label}.kind must be edit or write"
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
    message: String,
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
    json!({
        "repo": base.display().to_string(),
        "success": false,
        "checked": true,
        "applied": false,
        "files": files,
        "file_count": pending.len(),
        "diagnostics": [{
            "source": name,
            "severity": "error",
            "message": message,
        }],
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
    base_dir: &Path,
    max_bytes: usize,
    require_read: bool,
    allow_replace_all: bool,
    read_snapshots: &BTreeMap<PathBuf, SystemTime>,
) -> Result<Value, RuntimeError> {
    let input_path = required_input_string(name, input, "path")?;
    validate_git_pathspec(name, input_path)?;
    let operations = parse_file_edit_operations(name, input, allow_replace_all)?;
    let dry_run = optional_bool_input(name, input, "dry_run")?.unwrap_or(false);

    let base = canonicalize_tool_path(name, "base_dir", base_dir)?;
    let candidate = base.join(input_path);
    let path = canonicalize_tool_path(name, "input.path", &candidate)?;
    if !path.starts_with(&base) {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.path is outside configured base_dir"
        )));
    }
    if require_read {
        require_fresh_read(name, "input.path", "edit", &path, read_snapshots)?;
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
    if updated_bytes.len() > max_bytes {
        return Err(RuntimeError::Provider(format!(
            "tool {name} edited content exceeds max_bytes={max_bytes}"
        )));
    }
    let (diff_content, diff_truncated, diff_bytes) =
        bytes_to_limited_text(diff.as_bytes(), 64 * 1024);
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

    let repo = canonicalize_tool_path(name, "repo_dir", options.repo_dir)?;
    let changed = extract_patch_paths(name, &patch)?;
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
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| RuntimeError::Provider(format!("tool {name} system clock: {error}")))?
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "air-tool-patch-{}-{nonce}.patch",
        std::process::id()
    ));
    fs::write(&path, patch).map_err(|error| {
        RuntimeError::Provider(format!("tool {name} write temp patch: {error}"))
    })?;
    Ok(path)
}
