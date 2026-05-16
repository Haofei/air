use super::*;

pub(crate) fn call_repo_references_tool(
    name: &str,
    input: &Value,
    repo_dir: &Path,
    max_matches: usize,
    max_files: usize,
    context_lines: usize,
    max_bytes: usize,
) -> Result<Value, RuntimeError> {
    let symbol = required_input_string(name, input, "symbol")?.trim();
    if !is_identifier_like(symbol) {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.symbol must be an identifier-like token"
        )));
    }
    let effective_max_matches =
        optional_bounded_usize_input(name, input, "max_matches", max_matches)?
            .unwrap_or(max_matches);
    let effective_max_files =
        optional_bounded_usize_input(name, input, "max_files", max_files)?.unwrap_or(max_files);
    let effective_context_lines =
        optional_bounded_usize_input(name, input, "context_lines", context_lines)?
            .unwrap_or(context_lines);
    let repo = canonicalize_tool_path(name, "repo_dir", repo_dir)?;
    let paths = repo_tool_paths(name, input, &repo)?;
    let mut command = Command::new("rg");
    command.args([
        "--line-number",
        "--column",
        "--with-filename",
        "--no-heading",
        "--color",
        "never",
        "--fixed-strings",
        symbol,
    ]);
    if let Some(glob) = input.get("glob").and_then(Value::as_str) {
        validate_relative_path_filter(name, glob)?;
        command.arg("-g").arg(glob);
    }
    if !paths.is_empty() {
        command.args(&paths);
    }
    let output = command
        .current_dir(&repo)
        .output()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} repo references: {error}")))?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(RuntimeError::Provider(format!(
            "tool {name} repo references failed: {}",
            provider_error_snippet(&String::from_utf8_lossy(&output.stderr))
        )));
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    let mut references = Vec::new();
    let mut definitions = Vec::new();
    let mut match_lines_by_path: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut selected_paths = Vec::new();
    let mut token_match_count = 0usize;
    for line in raw.lines() {
        let item = parse_rg_vimgrep_line(line);
        let text = item["text"].as_str().unwrap_or_default();
        if !contains_identifier_token(text, symbol) {
            continue;
        }
        token_match_count += 1;
        if references.len() >= effective_max_matches {
            continue;
        }
        let path = item["path"].as_str().unwrap_or_default().to_string();
        let line_number = item["line"].as_u64().unwrap_or_default() as usize;
        let definition_kind = parse_symbol_declaration(text).and_then(|(kind, declaration)| {
            (kind != "impl" && declaration == symbol).then_some(kind)
        });
        let reference = build_reference_entry(&item, text, definition_kind);
        if definition_kind.is_some() {
            definitions.push(reference.clone());
        }
        references.push(reference);
        if path.is_empty() || line_number == 0 {
            continue;
        }
        if !match_lines_by_path.contains_key(&path) {
            if selected_paths.len() >= effective_max_files {
                continue;
            }
            selected_paths.push(path.clone());
        }
        match_lines_by_path
            .entry(path)
            .or_default()
            .push(line_number);
    }

    let mut snippets = Vec::new();
    let mut rendered = String::new();
    for path in selected_paths {
        let Some(lines) = match_lines_by_path.get(&path) else {
            continue;
        };
        let candidate = repo.join(&path);
        let file_path = canonicalize_tool_path(name, "repo reference path", &candidate)?;
        if !file_path.starts_with(&repo) {
            return Err(RuntimeError::Provider(format!(
                "tool {name} repo reference path is outside configured repo_dir"
            )));
        }
        let body = fs::read(&file_path)
            .map_err(|error| RuntimeError::Provider(format!("tool {name} read file: {error}")))?;
        let full_content = String::from_utf8_lossy(&body).to_string();
        let total_lines = full_content.lines().count();
        let ranges = merge_line_ranges(lines, total_lines, effective_context_lines);
        for (start_line, end_line) in ranges {
            let content = numbered_line_range(&full_content, start_line, end_line);
            if !rendered.is_empty() {
                rendered.push('\n');
            }
            rendered.push_str(&format!("--- {path}:{start_line}-{end_line} ---\n"));
            rendered.push_str(&content);
            snippets.push(json!({
                "path": path,
                "start_line": start_line,
                "end_line": end_line,
                "match_lines": lines
                    .iter()
                    .copied()
                    .filter(|line| *line >= start_line && *line <= end_line)
                    .collect::<Vec<_>>(),
                "content": content,
                "total_lines": total_lines,
            }));
        }
    }

    let (content, truncated_bytes, bytes) = bytes_to_limited_text(rendered.as_bytes(), max_bytes);
    let truncated = token_match_count > references.len() || truncated_bytes;
    Ok(json!({
        "repo": repo.display().to_string(),
        "symbol": symbol,
        "definitions": definitions,
        "references": references,
        "snippets": snippets,
        "bytes": bytes,
        "truncated": truncated,
        "artifacts": [{
            "id": format!("repo-references:{}:{}", repo.display(), symbol),
            "kind": "repo_references",
            "title": format!("repo references: {symbol}"),
            "uri": repo.display().to_string(),
            "content": content,
            "metadata": {
                "provider": "repo_references",
                "repo": repo.display().to_string(),
                "symbol": symbol,
                "paths": paths,
                "max_matches": effective_max_matches,
                "max_files": effective_max_files,
                "context_lines": effective_context_lines,
                "bytes": bytes,
                "truncated": truncated
            }
        }]
    }))
}
