use super::file_tools::{
    numbered_content, optional_bool_alias_input, optional_labeled_bool_alias_input,
    required_input_string_alias, required_labeled_string_alias, required_path_input,
};
use super::text_utils::{bytes_to_limited_text, merge_line_ranges, select_line_range};
use super::{
    canonicalize_tool_path, optional_bool_input, optional_bounded_usize_input,
    optional_labeled_string_input, optional_positive_usize_input,
};
use air_runtime::RuntimeError;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

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

pub(super) struct FileEditOptions<'a> {
    pub(super) base_dir: &'a Path,
    pub(super) max_bytes: usize,
    pub(super) max_changed_lines: Option<usize>,
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
        "workspace_changed": !dry_run,
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
                "workspace_changed": !dry_run,
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
