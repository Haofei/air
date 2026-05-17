use super::*;
use std::io::{BufRead, BufReader};

const GLOB_ALIASES: &[&str] = &["glob", "file_glob"];
const QUERY_ALIASES: &[&str] = &["query", "pattern"];
const SMALL_READ_LINE_THRESHOLD: usize = 200;
const SMALL_READ_BYTE_THRESHOLD: u64 = 20 * 1024;

struct RepoFilesQueryInput<'a> {
    raw_query: &'a str,
    query_source: &'static str,
    pattern_glob: Option<&'a str>,
}

fn repo_files_query_input<'a>(
    tool_name: &str,
    input: &'a Value,
) -> Result<RepoFilesQueryInput<'a>, RuntimeError> {
    if let Some(query) = input.get("query") {
        let pattern_glob = if input.get("glob").is_none() && input.get("file_glob").is_none() {
            input
                .get("pattern")
                .map(|pattern| {
                    let pattern = pattern.as_str().ok_or_else(|| {
                        RuntimeError::Provider(format!(
                            "tool {tool_name} input.pattern must be a string"
                        ))
                    })?;
                    let pattern = pattern.trim();
                    Ok(repo_files_pattern_looks_like_glob(pattern).then_some(pattern))
                })
                .transpose()?
                .flatten()
        } else {
            None
        };
        return query
            .as_str()
            .map(|q| RepoFilesQueryInput {
                raw_query: q.trim(),
                query_source: "query",
                pattern_glob,
            })
            .ok_or_else(|| {
                RuntimeError::Provider(format!("tool {tool_name} input.query must be a string"))
            });
    }
    if input.get("glob").is_none() && input.get("file_glob").is_none() {
        if let Some(pattern) = input.get("pattern") {
            let pattern = pattern.as_str().ok_or_else(|| {
                RuntimeError::Provider(format!("tool {tool_name} input.pattern must be a string"))
            })?;
            let pattern = pattern.trim();
            if repo_files_pattern_looks_like_glob(pattern) {
                return Ok(RepoFilesQueryInput {
                    raw_query: "",
                    query_source: "none",
                    pattern_glob: Some(pattern),
                });
            }
            return Ok(RepoFilesQueryInput {
                raw_query: pattern,
                query_source: "pattern",
                pattern_glob: None,
            });
        }
    }
    Ok(RepoFilesQueryInput {
        raw_query: "",
        query_source: "none",
        pattern_glob: None,
    })
}

struct RepoFilesGlobInput<'a> {
    glob: Option<&'a str>,
    glob_source: &'static str,
}

fn repo_files_glob_input<'a>(
    tool_name: &str,
    input: &'a Value,
    pattern_glob: Option<&'a str>,
) -> Result<RepoFilesGlobInput<'a>, RuntimeError> {
    if let Some((glob, glob_source)) = first_string_alias(tool_name, input, &["glob", "file_glob"])?
    {
        return Ok(RepoFilesGlobInput {
            glob: Some(glob),
            glob_source,
        });
    }
    Ok(RepoFilesGlobInput {
        glob: pattern_glob,
        glob_source: pattern_glob.map_or("none", |_| "pattern"),
    })
}

fn repo_files_pattern_looks_like_glob(pattern: &str) -> bool {
    pattern.contains('*')
        || pattern.contains('?')
        || pattern.contains('[')
        || pattern.contains('/')
        || pattern.contains('\\')
}

fn repo_file_match_score(path: &str, terms: &[String]) -> usize {
    if terms.is_empty() {
        return 0;
    }
    let lower = path.to_lowercase();
    let components = lower
        .split(['/', '.', '-', '_'])
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    terms
        .iter()
        .map(|term| {
            let contains_score = usize::from(lower.contains(term));
            let exact_score = components
                .iter()
                .filter(|component| **component == term)
                .count()
                * 2;
            contains_score + exact_score
        })
        .sum()
}

fn repo_files_no_matches_hint(
    query: &str,
    glob: Option<&str>,
    mode: &str,
    include_all: bool,
    path_scoped: bool,
) -> String {
    let mut hints = Vec::new();
    if glob.is_some() {
        hints.push("try a different glob or remove the glob scope");
    }
    if !query.trim().is_empty() {
        hints.push("broaden the query or use include_all=true to inspect nearby paths");
    }
    if mode != "smart" && !query.trim().is_empty() {
        hints.push("try mode=smart for natural-language file discovery");
    }
    if path_scoped {
        hints.push("check whether the path scope is too narrow");
    }
    if include_all {
        hints.push("inspect the project layout with an empty query or broader glob");
    }
    if hints.is_empty() {
        hints.push("broaden the file pattern or inspect the project layout");
    }
    format!("No files matched; {}.", hints.join(", "))
}

pub(crate) fn call_repo_files_tool(
    name: &str,
    input: &Value,
    repo_dir: &Path,
    max_files: usize,
) -> Result<Value, RuntimeError> {
    let repo = canonicalize_tool_path(name, "repo_dir", repo_dir)?;
    let query_input = repo_files_query_input(name, input)?;
    let mode = optional_string_input(name, input, "mode")?.unwrap_or("fixed");
    if !matches!(mode, "fixed" | "smart") {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.mode must be fixed or smart"
        )));
    }
    let query = query_input.raw_query.to_lowercase();
    let smart_terms = if mode == "smart" {
        repo_smart_search_terms(query_input.raw_query)
    } else {
        Vec::new()
    };
    let effective_query = if mode == "smart" {
        smart_terms.join("|")
    } else {
        query.clone()
    };
    let effective_max_files =
        optional_bounded_usize_input(name, input, "max_files", max_files)?.unwrap_or(max_files);
    let include_all = input
        .get("include_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let paths = repo_tool_paths(name, input, &repo)?;
    let glob_input = repo_files_glob_input(name, input, query_input.pattern_glob)?;
    let mut command = Command::new("rg");
    command.arg("--files");
    if let Some(glob) = glob_input.glob {
        validate_relative_path_filter(name, glob)?;
        command.arg("-g").arg(glob);
    }
    if !paths.is_empty() {
        command.args(&paths);
    }
    let output = command
        .current_dir(&repo)
        .output()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} repo files: {error}")))?;
    let no_matches =
        output.status.code() == Some(1) && output.stdout.is_empty() && output.stderr.is_empty();
    if !output.status.success() && !no_matches {
        return Err(RuntimeError::Provider(format!(
            "tool {name} repo files failed: {}",
            provider_error_snippet(&String::from_utf8_lossy(&output.stderr))
        )));
    }
    let mut candidates = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let path = line.trim();
        if path.is_empty() {
            continue;
        }
        let score = if mode == "smart" {
            repo_file_match_score(path, &smart_terms)
        } else {
            0
        };
        if mode == "smart" {
            if !include_all && !smart_terms.is_empty() && score == 0 {
                continue;
            }
        } else if !include_all && !query.is_empty() && !path.to_lowercase().contains(&query) {
            continue;
        }
        candidates.push((score, path.to_string()));
    }
    if mode == "smart" {
        candidates.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    }
    let truncated = candidates.len() > effective_max_files;
    let files = candidates
        .into_iter()
        .take(effective_max_files)
        .map(|(_, path)| path)
        .collect::<Vec<_>>();
    let file_infos = files
        .iter()
        .map(|path| repo_file_info(&repo, path))
        .collect::<Vec<_>>();
    let content = files.join("\n");
    let file_count = files.len();
    let no_matches = file_count == 0;
    let search_hint = no_matches.then(|| {
        repo_files_no_matches_hint(
            query_input.raw_query,
            glob_input.glob,
            mode,
            include_all,
            !paths.is_empty(),
        )
    });
    Ok(json!({
        "repo": repo.display().to_string(),
        "query": query_input.raw_query,
        "query_source": query_input.query_source,
        "effective_query": effective_query,
        "glob": glob_input.glob,
        "glob_source": glob_input.glob_source,
        "mode": mode,
        "files": files,
        "file_infos": file_infos.clone(),
        "file_count": file_count,
        "no_matches": no_matches,
        "search_hint": search_hint,
        "include_all": include_all,
        "truncated": truncated,
        "artifacts": [{
            "id": format!("repo-files:{}:{}", repo.display(), query_input.raw_query),
            "kind": "repo_listing",
            "title": "repo files",
            "uri": repo.display().to_string(),
            "content": content,
            "metadata": {
                "provider": "repo_files",
                "repo": repo.display().to_string(),
                "query": query_input.raw_query,
                "query_source": query_input.query_source,
                "effective_query": effective_query,
                "glob": glob_input.glob,
                "glob_source": glob_input.glob_source,
                "mode": mode,
                "max_files": effective_max_files,
                "include_all": include_all,
                "file_count": file_count,
                "no_matches": no_matches,
                "search_hint": search_hint,
                "file_infos": file_infos
            }
        }]
    }))
}

fn repo_file_info(repo: &Path, relative_path: &str) -> Value {
    let path = repo.join(relative_path);
    let source_bytes = fs::metadata(&path).ok().map(|metadata| metadata.len());
    let line_count = count_file_lines(&path).ok();
    let large = source_bytes
        .map(|bytes| bytes > SMALL_READ_BYTE_THRESHOLD)
        .unwrap_or(false)
        || line_count
            .map(|lines| lines > SMALL_READ_LINE_THRESHOLD)
            .unwrap_or(false);
    json!({
        "path": relative_path,
        "source_bytes": source_bytes,
        "line_count": line_count,
        "large": large,
        "whole_read_ok": !large,
        "read_guidance": if large {
            "large file: use grep, contains+context_lines, LSP, or a narrow line range before Read"
        } else {
            "small file: whole-file Read is reasonable"
        }
    })
}

fn count_file_lines(path: &Path) -> Result<usize, RuntimeError> {
    let file = fs::File::open(path)
        .map_err(|error| RuntimeError::Provider(format!("count file lines: {error}")))?;
    let mut reader = BufReader::new(file);
    let mut buf = Vec::new();
    let mut lines = 0usize;
    let mut saw_bytes = false;
    loop {
        buf.clear();
        let bytes = reader
            .read_until(b'\n', &mut buf)
            .map_err(|error| RuntimeError::Provider(format!("count file lines: {error}")))?;
        if bytes == 0 {
            break;
        }
        saw_bytes = true;
        lines += 1;
    }
    Ok(if saw_bytes { lines } else { 0 })
}

fn repo_search_query_input<'a>(
    tool_name: &str,
    input: &'a Value,
) -> Result<(&'a str, &'static str), RuntimeError> {
    first_string_alias(tool_name, input, QUERY_ALIASES)?.ok_or_else(|| {
        RuntimeError::Provider(format!("tool {tool_name} input.query must be a string"))
    })
}

fn repo_glob_input<'a>(tool_name: &str, input: &'a Value) -> Result<Option<&'a str>, RuntimeError> {
    Ok(first_string_alias(tool_name, input, GLOB_ALIASES)?.map(|(v, _)| v))
}

fn repo_effective_search_query(mode: &str, query: &str) -> String {
    if mode != "smart" {
        return query.to_string();
    }
    let terms = repo_smart_search_terms(query);
    if terms.is_empty() {
        return regex::escape(query);
    }
    format!(
        r"\b(?:{})\b",
        terms
            .iter()
            .map(|term| regex::escape(term))
            .collect::<Vec<_>>()
            .join("|")
    )
}

fn run_repo_rg(
    name: &str,
    repo: &Path,
    paths: &[String],
    glob: Option<&str>,
    mode: &str,
    effective_query: &str,
    label: &str,
) -> Result<std::process::Output, RuntimeError> {
    let mut command = Command::new("rg");
    command.args([
        "--line-number",
        "--column",
        "--with-filename",
        "--no-heading",
        "--color",
        "never",
    ]);
    if mode == "fixed" {
        command.arg("--fixed-strings");
    }
    if mode == "smart" {
        command.arg("--ignore-case");
    }
    command.arg(effective_query);
    if let Some(glob) = glob {
        command.arg("-g").arg(glob);
    }
    if !paths.is_empty() {
        command.args(paths);
    }
    command
        .current_dir(repo)
        .output()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} {label}: {error}")))
}

fn should_smart_fallback(mode: &str, query: &str) -> bool {
    mode == "fixed" && repo_smart_search_terms(query).len() >= 2
}

fn ensure_repo_rg_success(
    name: &str,
    label: &str,
    output: &std::process::Output,
) -> Result<(), RuntimeError> {
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(RuntimeError::Provider(format!(
            "tool {name} {label} failed: {}",
            provider_error_snippet(&String::from_utf8_lossy(&output.stderr))
        )));
    }
    Ok(())
}

fn run_repo_rg_with_smart_fallback(
    name: &str,
    repo: &Path,
    paths: &[String],
    glob: Option<&str>,
    requested_mode: &str,
    query: &str,
    label: &str,
) -> Result<(String, String, std::process::Output), RuntimeError> {
    let mut mode = requested_mode.to_string();
    let mut effective_query = repo_effective_search_query(&mode, query);
    let mut output = run_repo_rg(name, repo, paths, glob, &mode, &effective_query, label)?;
    ensure_repo_rg_success(name, label, &output)?;
    if output.status.code() == Some(1) && should_smart_fallback(requested_mode, query) {
        mode = "smart".to_string();
        effective_query = repo_effective_search_query(&mode, query);
        output = run_repo_rg(name, repo, paths, glob, &mode, &effective_query, label)?;
        ensure_repo_rg_success(name, label, &output)?;
    }
    Ok((mode, effective_query, output))
}

fn repo_search_no_matches_hint(
    query: &str,
    glob: Option<&str>,
    paths: &[String],
    mode: &str,
) -> String {
    let mut hints = Vec::new();
    if glob.is_some() {
        hints.push("remove or broaden the glob scope");
    }
    if !paths.is_empty() {
        hints.push("check whether the path scope is too narrow");
    }
    if mode != "smart" && repo_smart_search_terms(query).len() >= 2 {
        hints.push("try mode=smart for the same query");
    }
    hints.push("broaden the query or search for a concrete symbol/path fragment");
    format!("No content matches found; {}.", hints.join(", "))
}

pub(crate) fn call_repo_search_tool(
    name: &str,
    input: &Value,
    repo_dir: &Path,
    max_matches: usize,
    max_bytes: usize,
) -> Result<Value, RuntimeError> {
    let (query, query_source) = repo_search_query_input(name, input)?;
    let requested_mode = optional_string_input(name, input, "mode")?.unwrap_or("fixed");
    if !matches!(requested_mode, "fixed" | "regex" | "smart") {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.mode must be fixed, regex, or smart"
        )));
    }
    let effective_max_matches =
        optional_bounded_usize_input(name, input, "max_matches", max_matches)?
            .unwrap_or(max_matches);
    let repo = canonicalize_tool_path(name, "repo_dir", repo_dir)?;
    let paths = repo_tool_paths(name, input, &repo)?;
    let glob = repo_glob_input(name, input)?;
    if let Some(glob) = glob {
        validate_relative_path_filter(name, glob)?;
    }
    let (mode, effective_query, output) = run_repo_rg_with_smart_fallback(
        name,
        &repo,
        &paths,
        glob,
        requested_mode,
        query,
        "repo search",
    )?;
    let raw = String::from_utf8_lossy(&output.stdout);
    let mut matches = Vec::new();
    for line in raw.lines().take(effective_max_matches) {
        matches.push(parse_rg_vimgrep_line(line));
    }
    let rendered = matches
        .iter()
        .map(|item| {
            format!(
                "{}:{}:{}:{}",
                item["path"].as_str().unwrap_or_default(),
                item["line"].as_u64().unwrap_or_default(),
                item["column"].as_u64().unwrap_or_default(),
                item["text"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let (content, truncated_bytes, bytes) = bytes_to_limited_text(rendered.as_bytes(), max_bytes);
    let match_count = raw.lines().count();
    let truncated = match_count > matches.len() || truncated_bytes;
    let no_matches = match_count == 0;
    let search_hint = no_matches.then(|| repo_search_no_matches_hint(query, glob, &paths, &mode));
    Ok(json!({
        "repo": repo.display().to_string(),
        "query": query,
        "query_source": query_source,
        "effective_query": effective_query,
        "glob": glob,
        "mode": mode,
        "requested_mode": requested_mode,
        "matches": matches,
        "match_count": match_count,
        "no_matches": no_matches,
        "search_hint": search_hint,
        "bytes": bytes,
        "truncated": truncated,
        "artifacts": [{
            "id": format!("repo-search:{}:{}", repo.display(), query),
            "kind": "repo_search",
            "title": format!("repo search: {query}"),
            "uri": repo.display().to_string(),
            "content": content,
            "metadata": {
                "provider": "repo_search",
                "repo": repo.display().to_string(),
                "query": query,
                "query_source": query_source,
                "effective_query": effective_query,
                "glob": glob,
                "mode": mode,
                "requested_mode": requested_mode,
                "paths": paths,
                "match_count": match_count,
                "no_matches": no_matches,
                "search_hint": search_hint,
                "max_matches": effective_max_matches,
                "bytes": bytes,
                "truncated": truncated
            }
        }]
    }))
}

pub(crate) fn call_repo_context_tool(
    name: &str,
    input: &Value,
    repo_dir: &Path,
    max_matches: usize,
    max_files: usize,
    context_lines: usize,
    max_bytes: usize,
) -> Result<Value, RuntimeError> {
    let query = required_input_string(name, input, "query")?;
    let requested_mode = optional_string_input(name, input, "mode")?.unwrap_or("fixed");
    if !matches!(requested_mode, "fixed" | "regex" | "smart") {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.mode must be fixed, regex, or smart"
        )));
    }
    let repo = canonicalize_tool_path(name, "repo_dir", repo_dir)?;
    let paths = repo_tool_paths(name, input, &repo)?;
    let effective_max_matches =
        optional_bounded_usize_input(name, input, "max_matches", max_matches)?
            .unwrap_or(max_matches);
    let effective_max_files =
        optional_bounded_usize_input(name, input, "max_files", max_files)?.unwrap_or(max_files);
    let effective_context_lines =
        optional_bounded_usize_input(name, input, "context_lines", context_lines)?
            .unwrap_or(context_lines);

    let glob = repo_glob_input(name, input)?;
    if let Some(glob) = glob {
        validate_relative_path_filter(name, glob)?;
    }
    let (mode, effective_query, output) = run_repo_rg_with_smart_fallback(
        name,
        &repo,
        &paths,
        glob,
        requested_mode,
        query,
        "repo context",
    )?;

    let raw = String::from_utf8_lossy(&output.stdout);
    let raw_line_count = raw.lines().count();
    let mut matches = Vec::new();
    let mut match_lines_by_path: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut selected_paths = Vec::new();
    for line in raw.lines().take(effective_max_matches) {
        let item = parse_rg_vimgrep_line(line);
        let path = item["path"].as_str().unwrap_or_default().to_string();
        let line_number = item["line"].as_u64().unwrap_or_default() as usize;
        matches.push(item);
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
        let file_path = canonicalize_tool_path(name, "repo context path", &candidate)?;
        if !file_path.starts_with(&repo) {
            return Err(RuntimeError::Provider(format!(
                "tool {name} repo context path is outside configured repo_dir"
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
    let truncated = raw_line_count > matches.len() || truncated_bytes;
    let no_matches = raw_line_count == 0;
    let search_hint = no_matches.then(|| repo_search_no_matches_hint(query, glob, &paths, &mode));
    Ok(json!({
        "repo": repo.display().to_string(),
        "query": query,
        "effective_query": effective_query,
        "mode": mode,
        "requested_mode": requested_mode,
        "matches": matches,
        "match_count": raw_line_count,
        "no_matches": no_matches,
        "search_hint": search_hint,
        "snippets": snippets,
        "bytes": bytes,
        "truncated": truncated,
        "artifacts": [{
            "id": format!("repo-context:{}:{}", repo.display(), query),
            "kind": "code_context",
            "title": format!("repo context: {query}"),
            "uri": repo.display().to_string(),
            "content": content,
            "metadata": {
                "provider": "repo_context",
                "repo": repo.display().to_string(),
                "query": query,
                "effective_query": effective_query,
                "mode": mode,
                "requested_mode": requested_mode,
                "paths": paths,
                "match_count": raw_line_count,
                "no_matches": no_matches,
                "search_hint": search_hint,
                "max_matches": effective_max_matches,
                "max_files": effective_max_files,
                "context_lines": effective_context_lines,
                "bytes": bytes,
                "truncated": truncated
            }
        }]
    }))
}
