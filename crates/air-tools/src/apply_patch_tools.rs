use super::canonicalize_tool_path;
use super::file_tools::required_input_string_alias;
use super::text_utils::bytes_to_limited_text;
use air_runtime::RuntimeError;
use serde_json::{json, Value};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub(super) struct ApplyPatchOptions<'a> {
    pub(super) base_dir: &'a Path,
    pub(super) max_bytes: usize,
}

#[derive(Debug, Clone)]
enum PatchHunk {
    Add {
        path: String,
        content: String,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        move_path: Option<String>,
        chunks: Vec<UpdateChunk>,
    },
}

#[derive(Debug, Clone)]
struct UpdateChunk {
    change_context: Option<String>,
    old_lines: Vec<String>,
    new_lines: Vec<String>,
    is_end_of_file: bool,
}

#[derive(Debug)]
struct FileChange {
    path: PathBuf,
    relative_path: String,
    kind: &'static str,
    new_content: String,
    move_path: Option<PathBuf>,
    move_relative_path: Option<String>,
    diff: String,
    additions: usize,
    deletions: usize,
}

pub(super) fn call_apply_patch_tool(
    name: &str,
    input: &Value,
    options: ApplyPatchOptions<'_>,
) -> Result<Value, RuntimeError> {
    let patch_text = required_input_string_alias(name, input, "patchText", "patch_text")?;
    if patch_text.len() > options.max_bytes {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.patchText exceeds max_bytes={}",
            options.max_bytes
        )));
    }
    let base = canonicalize_tool_path(name, "base_dir", options.base_dir)?;
    let hunks = parse_patch(name, patch_text)?;
    if hunks.is_empty() {
        return Err(RuntimeError::Provider(format!(
            "tool {name} rejected empty patch"
        )));
    }

    let mut changes = Vec::new();
    for hunk in hunks {
        changes.push(plan_change(name, &base, hunk)?);
    }

    let mut total_diff = String::new();
    for change in &changes {
        total_diff.push_str(&change.diff);
        if !total_diff.ends_with('\n') {
            total_diff.push('\n');
        }
    }

    for change in &changes {
        match change.kind {
            "add" => {
                let parent = change.path.parent().ok_or_else(|| {
                    RuntimeError::Provider(format!(
                        "tool {name} patch target must have a parent directory"
                    ))
                })?;
                fs::create_dir_all(parent).map_err(|error| {
                    RuntimeError::Provider(format!(
                        "tool {name} create parent directories: {error}"
                    ))
                })?;
                fs::write(&change.path, change.new_content.as_bytes()).map_err(|error| {
                    RuntimeError::Provider(format!("tool {name} write file: {error}"))
                })?;
            }
            "update" => {
                fs::write(&change.path, change.new_content.as_bytes()).map_err(|error| {
                    RuntimeError::Provider(format!("tool {name} write file: {error}"))
                })?;
            }
            "move" => {
                let move_path = change.move_path.as_ref().ok_or_else(|| {
                    RuntimeError::Provider(format!("tool {name} move patch missing target path"))
                })?;
                let parent = move_path.parent().ok_or_else(|| {
                    RuntimeError::Provider(format!(
                        "tool {name} patch move target must have a parent directory"
                    ))
                })?;
                fs::create_dir_all(parent).map_err(|error| {
                    RuntimeError::Provider(format!(
                        "tool {name} create parent directories: {error}"
                    ))
                })?;
                fs::write(move_path, change.new_content.as_bytes()).map_err(|error| {
                    RuntimeError::Provider(format!("tool {name} write moved file: {error}"))
                })?;
                fs::remove_file(&change.path).map_err(|error| {
                    RuntimeError::Provider(format!("tool {name} remove moved source: {error}"))
                })?;
            }
            "delete" => {
                fs::remove_file(&change.path).map_err(|error| {
                    RuntimeError::Provider(format!("tool {name} delete file: {error}"))
                })?;
            }
            _ => unreachable!("validated file change kind"),
        }
    }

    let (diff, diff_truncated, diff_bytes) =
        bytes_to_limited_text(total_diff.as_bytes(), 64 * 1024);
    let files = changes
        .iter()
        .map(|change| {
            json!({
                "path": change.relative_path,
                "absolute_path": change.path.display().to_string(),
                "kind": change.kind,
                "move_path": change.move_relative_path,
                "move_absolute_path": change.move_path.as_ref().map(|path| path.display().to_string()),
                "additions": change.additions,
                "deletions": change.deletions
            })
        })
        .collect::<Vec<_>>();
    let summary = build_apply_patch_summary(&changes);

    Ok(json!({
        "repo": base.display().to_string(),
        "success": true,
        "checked": true,
        "applied": true,
        "workspace_changed": true,
        "files": files,
        "file_count": changes.len(),
        "diagnostics": [],
        "diff": diff,
        "diff_truncated": diff_truncated,
        "diff_bytes": diff_bytes,
        "output": build_apply_patch_output(&summary),
        "artifacts": [{
            "id": format!("apply-patch:{}", stable_change_id(&summary)),
            "kind": "file_edit",
            "title": "apply_patch",
            "uri": base.display().to_string(),
            "content": diff,
            "metadata": {
                "provider": "apply_patch",
                "repo": base.display().to_string(),
                "success": true,
                "checked": true,
                "applied": true,
                "workspace_changed": true,
                "file_count": changes.len(),
                "diff_bytes": diff_bytes,
                "diff_truncated": diff_truncated,
                "summary": summary
            }
        }]
    }))
}

fn parse_patch(tool_name: &str, patch_text: &str) -> Result<Vec<PatchHunk>, RuntimeError> {
    let cleaned = strip_heredoc(patch_text.trim())
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let lines = cleaned.lines().map(str::to_string).collect::<Vec<_>>();
    let begin = lines
        .iter()
        .position(|line| line.trim() == "*** Begin Patch")
        .ok_or_else(|| {
            RuntimeError::Provider(format!(
                "tool {tool_name} apply_patch verification failed: missing Begin marker"
            ))
        })?;
    let end = lines
        .iter()
        .position(|line| line.trim() == "*** End Patch")
        .ok_or_else(|| {
            RuntimeError::Provider(format!(
                "tool {tool_name} apply_patch verification failed: missing End marker"
            ))
        })?;
    if begin >= end {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} apply_patch verification failed: Begin marker must precede End marker"
        )));
    }

    let mut hunks = Vec::new();
    let mut index = begin + 1;
    while index < end {
        let line = &lines[index];
        if let Some(path) = line.strip_prefix("*** Add File:") {
            let (content, next_index) = parse_add_file_content(&lines, index + 1, end);
            hunks.push(PatchHunk::Add {
                path: clean_patch_path(path),
                content,
            });
            index = next_index;
        } else if let Some(path) = line.strip_prefix("*** Delete File:") {
            hunks.push(PatchHunk::Delete {
                path: clean_patch_path(path),
            });
            index += 1;
        } else if let Some(path) = line.strip_prefix("*** Update File:") {
            let mut next_index = index + 1;
            let mut move_path = None;
            if next_index < end {
                if let Some(target) = lines[next_index].strip_prefix("*** Move to:") {
                    move_path = Some(clean_patch_path(target));
                    next_index += 1;
                }
            }
            let (chunks, chunk_end) = parse_update_chunks(&lines, next_index, end);
            hunks.push(PatchHunk::Update {
                path: clean_patch_path(path),
                move_path,
                chunks,
            });
            index = chunk_end;
        } else {
            index += 1;
        }
    }
    Ok(hunks)
}

fn build_apply_patch_summary(changes: &[FileChange]) -> Vec<String> {
    changes
        .iter()
        .map(|change| match change.kind {
            "add" => format!("A {}", change.relative_path),
            "delete" => format!("D {}", change.relative_path),
            "move" => format!(
                "M {}",
                change
                    .move_relative_path
                    .as_deref()
                    .unwrap_or(change.relative_path.as_str())
            ),
            _ => format!("M {}", change.relative_path),
        })
        .collect()
}

fn build_apply_patch_output(summary: &[String]) -> String {
    format!(
        "Success. Updated the following files:\n{}",
        summary.join("\n")
    )
}

fn strip_heredoc(text: &str) -> String {
    let lines = text.lines().collect::<Vec<_>>();
    let Some(first) = lines.first() else {
        return text.to_string();
    };
    let marker = first
        .trim()
        .strip_prefix("cat <<")
        .or_else(|| first.trim().strip_prefix("<<"))
        .map(|value| value.trim_matches(|ch| ch == '\'' || ch == '"' || ch == ' '));
    let Some(marker) = marker else {
        return text.to_string();
    };
    if marker.is_empty() || lines.last().copied() != Some(marker) || lines.len() < 3 {
        return text.to_string();
    }
    lines[1..lines.len() - 1].join("\n")
}

fn clean_patch_path(path: &str) -> String {
    path.trim().to_string()
}

fn parse_add_file_content(lines: &[String], start: usize, end: usize) -> (String, usize) {
    let mut content = String::new();
    let mut index = start;
    while index < end && !lines[index].starts_with("***") {
        if let Some(line) = lines[index].strip_prefix('+') {
            content.push_str(line);
            content.push('\n');
        }
        index += 1;
    }
    if content.ends_with('\n') {
        content.pop();
    }
    (content, index)
}

fn parse_update_chunks(lines: &[String], start: usize, end: usize) -> (Vec<UpdateChunk>, usize) {
    let mut chunks = Vec::new();
    let mut index = start;
    while index < end && !lines[index].starts_with("***") {
        if !lines[index].starts_with("@@") {
            index += 1;
            continue;
        }
        let context = lines[index]
            .strip_prefix("@@")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        index += 1;

        let mut old_lines = Vec::new();
        let mut new_lines = Vec::new();
        let mut is_end_of_file = false;
        while index < end && !lines[index].starts_with("@@") && !lines[index].starts_with("***") {
            let line = &lines[index];
            if line == "*** End of File" {
                is_end_of_file = true;
                index += 1;
                break;
            }
            if let Some(value) = line.strip_prefix(' ') {
                old_lines.push(value.to_string());
                new_lines.push(value.to_string());
            } else if let Some(value) = line.strip_prefix('-') {
                old_lines.push(value.to_string());
            } else if let Some(value) = line.strip_prefix('+') {
                new_lines.push(value.to_string());
            }
            index += 1;
        }
        chunks.push(UpdateChunk {
            change_context: context,
            old_lines,
            new_lines,
            is_end_of_file,
        });
    }
    (chunks, index)
}

fn plan_change(tool_name: &str, base: &Path, hunk: PatchHunk) -> Result<FileChange, RuntimeError> {
    match hunk {
        PatchHunk::Add { path, content } => {
            let target = resolve_patch_path(tool_name, base, &path)?;
            if target.exists() {
                return Err(RuntimeError::Provider(format!(
                    "tool {tool_name} apply_patch verification failed: file already exists: {path}"
                )));
            }
            let new_content = ensure_trailing_newline(content);
            let relative_path = display_patch_path(base, &target, &path);
            let diff = simple_unified_diff(&relative_path, "", &new_content);
            let (additions, deletions) = count_changed_lines("", &new_content);
            Ok(FileChange {
                path: target,
                relative_path,
                kind: "add",
                new_content,
                move_path: None,
                move_relative_path: None,
                diff,
                additions,
                deletions,
            })
        }
        PatchHunk::Delete { path } => {
            let target = resolve_existing_patch_path(tool_name, base, &path)?;
            let old_content = read_text_file(tool_name, &target)?;
            let relative_path = display_patch_path(base, &target, &path);
            let diff = simple_unified_diff(&relative_path, &old_content, "");
            let (additions, deletions) = count_changed_lines(&old_content, "");
            Ok(FileChange {
                path: target,
                relative_path,
                kind: "delete",
                new_content: String::new(),
                move_path: None,
                move_relative_path: None,
                diff,
                additions,
                deletions,
            })
        }
        PatchHunk::Update {
            path,
            move_path,
            chunks,
        } => {
            let target = resolve_existing_patch_path(tool_name, base, &path)?;
            let old_content = read_text_file(tool_name, &target)?;
            let new_content = derive_new_content(tool_name, &path, &old_content, &chunks)?;
            let resolved_move_path = move_path
                .as_deref()
                .map(|path| resolve_patch_path(tool_name, base, path))
                .transpose()?;
            let relative_path = display_patch_path(base, &target, &path);
            let move_relative_path = resolved_move_path
                .as_ref()
                .zip(move_path.as_deref())
                .map(|(path, fallback)| display_patch_path(base, path, fallback));
            let diff_path = move_relative_path.as_deref().unwrap_or(&relative_path);
            let diff = simple_unified_diff(diff_path, &old_content, &new_content);
            let (additions, deletions) = count_changed_lines(&old_content, &new_content);
            Ok(FileChange {
                path: target,
                relative_path,
                kind: if move_path.is_some() {
                    "move"
                } else {
                    "update"
                },
                new_content,
                move_path: resolved_move_path,
                move_relative_path,
                diff,
                additions,
                deletions,
            })
        }
    }
}

fn resolve_patch_path(
    tool_name: &str,
    base: &Path,
    input_path: &str,
) -> Result<PathBuf, RuntimeError> {
    if input_path.trim().is_empty() {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} patch path must not be empty"
        )));
    }
    let candidate = if Path::new(input_path).is_absolute() {
        validate_no_parent_patch_components(tool_name, input_path)?;
        PathBuf::from(input_path)
    } else {
        validate_relative_patch_path(tool_name, input_path)?;
        base.join(input_path)
    };
    if !candidate.starts_with(base) {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} patch path is outside configured base_dir"
        )));
    }
    let parent = candidate.parent().ok_or_else(|| {
        RuntimeError::Provider(format!(
            "tool {tool_name} patch path must have a parent directory"
        ))
    })?;
    if parent.exists() {
        let parent = canonicalize_tool_path(tool_name, "patch path parent", parent)?;
        if !parent.starts_with(base) {
            return Err(RuntimeError::Provider(format!(
                "tool {tool_name} patch path is outside configured base_dir"
            )));
        }
    }
    let _ = patch_file_name(tool_name, &candidate)?;
    Ok(candidate)
}

fn validate_relative_patch_path(tool_name: &str, input_path: &str) -> Result<(), RuntimeError> {
    let path = Path::new(input_path);
    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(RuntimeError::Provider(format!(
                    "tool {tool_name} patch path is outside configured base_dir"
                )));
            }
        }
    }
    if path.file_name().is_none() {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} patch path must include a file name"
        )));
    }
    Ok(())
}

fn validate_no_parent_patch_components(
    tool_name: &str,
    input_path: &str,
) -> Result<(), RuntimeError> {
    if Path::new(input_path)
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} patch path is outside configured base_dir"
        )));
    }
    Ok(())
}

fn patch_file_name<'a>(
    tool_name: &str,
    path: &'a Path,
) -> Result<&'a std::ffi::OsStr, RuntimeError> {
    path.file_name().ok_or_else(|| {
        RuntimeError::Provider(format!(
            "tool {tool_name} patch path must include a file name"
        ))
    })
}

fn resolve_existing_patch_path(
    tool_name: &str,
    base: &Path,
    input_path: &str,
) -> Result<PathBuf, RuntimeError> {
    let candidate = if Path::new(input_path).is_absolute() {
        PathBuf::from(input_path)
    } else {
        base.join(input_path)
    };
    let path = canonicalize_tool_path(tool_name, "patch path", &candidate)?;
    if !path.starts_with(base) {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} patch path is outside configured base_dir"
        )));
    }
    Ok(path)
}

fn read_text_file(tool_name: &str, path: &Path) -> Result<String, RuntimeError> {
    fs::read_to_string(path)
        .map_err(|error| RuntimeError::Provider(format!("tool {tool_name} read file: {error}")))
}

fn derive_new_content(
    tool_name: &str,
    file_path: &str,
    old_content: &str,
    chunks: &[UpdateChunk],
) -> Result<String, RuntimeError> {
    let mut original_lines = old_content
        .split('\n')
        .map(str::to_string)
        .collect::<Vec<_>>();
    if original_lines.last().is_some_and(String::is_empty) {
        original_lines.pop();
    }
    let replacements = compute_replacements(tool_name, file_path, &original_lines, chunks)?;
    let mut new_lines = original_lines;
    for (start, old_len, replacement) in replacements.into_iter().rev() {
        new_lines.splice(start..start + old_len, replacement);
    }
    if new_lines.last().is_none_or(|line| !line.is_empty()) {
        new_lines.push(String::new());
    }
    Ok(new_lines.join("\n"))
}

fn compute_replacements(
    tool_name: &str,
    file_path: &str,
    original_lines: &[String],
    chunks: &[UpdateChunk],
) -> Result<Vec<(usize, usize, Vec<String>)>, RuntimeError> {
    let mut replacements = Vec::new();
    let mut line_index = 0usize;
    for chunk in chunks {
        if let Some(context) = &chunk.change_context {
            let context_index =
                seek_sequence(original_lines, &[context.to_string()], line_index, false);
            if let Some(context_index) = context_index {
                line_index = context_index + 1;
            } else {
                return Err(RuntimeError::Provider(format!(
                    "tool {tool_name} apply_patch verification failed: failed to find context '{context}' in {file_path}"
                )));
            }
        }
        if chunk.old_lines.is_empty() {
            let insertion = if chunk.is_end_of_file {
                original_lines.len()
            } else {
                line_index
            };
            replacements.push((insertion, 0, chunk.new_lines.clone()));
            continue;
        }

        let mut pattern = chunk.old_lines.clone();
        let mut new_slice = chunk.new_lines.clone();
        let mut found = seek_sequence(original_lines, &pattern, line_index, chunk.is_end_of_file);
        if found.is_none() && pattern.last().is_some_and(String::is_empty) {
            pattern.pop();
            if new_slice.last().is_some_and(String::is_empty) {
                new_slice.pop();
            }
            found = seek_sequence(original_lines, &pattern, line_index, chunk.is_end_of_file);
        }
        let Some(found) = found else {
            return Err(RuntimeError::Provider(format!(
                "tool {tool_name} apply_patch verification failed: failed to find expected lines in {file_path}:\n{}",
                chunk.old_lines.join("\n")
            )));
        };
        replacements.push((found, pattern.len(), new_slice));
        line_index = found + pattern.len();
    }
    replacements.sort_by_key(|(start, _, _)| *start);
    Ok(replacements)
}

fn seek_sequence(
    lines: &[String],
    pattern: &[String],
    start_index: usize,
    end_of_file: bool,
) -> Option<usize> {
    if pattern.is_empty() || pattern.len() > lines.len() {
        return None;
    }
    for comparator in [
        same_exact as fn(&str, &str) -> bool,
        same_trim_end,
        same_trim,
        same_normalized,
    ] {
        if let Some(index) = try_match(lines, pattern, start_index, comparator, end_of_file) {
            return Some(index);
        }
    }
    None
}

fn try_match(
    lines: &[String],
    pattern: &[String],
    start_index: usize,
    compare: fn(&str, &str) -> bool,
    end_of_file: bool,
) -> Option<usize> {
    if end_of_file {
        let from_end = lines.len().checked_sub(pattern.len())?;
        if from_end >= start_index && matches_at(lines, pattern, from_end, compare) {
            return Some(from_end);
        }
    }
    (start_index..=lines.len().saturating_sub(pattern.len()))
        .find(|index| matches_at(lines, pattern, *index, compare))
}

fn matches_at(
    lines: &[String],
    pattern: &[String],
    start: usize,
    compare: fn(&str, &str) -> bool,
) -> bool {
    pattern
        .iter()
        .enumerate()
        .all(|(offset, expected)| compare(&lines[start + offset], expected))
}

fn same_exact(left: &str, right: &str) -> bool {
    left == right
}

fn same_trim_end(left: &str, right: &str) -> bool {
    left.trim_end() == right.trim_end()
}

fn same_trim(left: &str, right: &str) -> bool {
    left.trim() == right.trim()
}

fn same_normalized(left: &str, right: &str) -> bool {
    normalize_unicode(left.trim()) == normalize_unicode(right.trim())
}

fn normalize_unicode(text: &str) -> String {
    text.replace(['\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}'], "'")
        .replace(['\u{201C}', '\u{201D}', '\u{201E}', '\u{201F}'], "\"")
        .replace(
            [
                '\u{2010}', '\u{2011}', '\u{2012}', '\u{2013}', '\u{2014}', '\u{2015}',
            ],
            "-",
        )
        .replace('\u{2026}', "...")
        .replace('\u{00A0}', " ")
}

fn ensure_trailing_newline(mut content: String) -> String {
    if !content.ends_with('\n') {
        content.push('\n');
    }
    content
}

fn display_patch_path(base: &Path, path: &Path, fallback: &str) -> String {
    path.strip_prefix(base)
        .ok()
        .and_then(|path| path.to_str())
        .filter(|path| !path.is_empty())
        .unwrap_or(fallback)
        .replace('\\', "/")
}

fn simple_unified_diff(path: &str, old_content: &str, new_content: &str) -> String {
    let mut diff = format!("--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n");
    for line in old_content.lines() {
        diff.push('-');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in new_content.lines() {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    diff
}

fn count_changed_lines(old_content: &str, new_content: &str) -> (usize, usize) {
    (new_content.lines().count(), old_content.lines().count())
}

fn stable_change_id(lines: &[String]) -> String {
    let mut hash: u64 = 14_695_981_039_346_656_037;
    for line in lines {
        for byte in line.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(1_099_511_628_211);
        }
    }
    format!("{hash:016x}")
}
