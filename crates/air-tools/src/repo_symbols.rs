use air_runtime::RuntimeError;
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::Command;

pub(crate) fn call_repo_symbols_tool(
    name: &str,
    input: &Value,
    repo_dir: &Path,
    max_symbols: usize,
    max_bytes: usize,
) -> Result<Value, RuntimeError> {
    let raw_query = input
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let names = repo_symbol_names_input(name, input)?;
    let name_filters = names
        .iter()
        .map(|name| name.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let mode = super::optional_string_input(name, input, "mode")?.unwrap_or("fixed");
    if !matches!(mode, "fixed" | "smart") {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.mode must be fixed or smart"
        )));
    }
    let query = raw_query.to_lowercase();
    let smart_terms = if mode == "smart" {
        super::repo_smart_search_terms(raw_query)
    } else {
        Vec::new()
    };
    let fixed_terms = if mode == "fixed" {
        fixed_symbol_query_terms(raw_query)
    } else {
        Vec::new()
    };
    let effective_query = if mode == "smart" {
        smart_terms.join("|")
    } else if !names.is_empty() {
        names.join("|")
    } else if !fixed_terms.is_empty() {
        fixed_terms.join("|")
    } else {
        query.clone()
    };
    let effective_max_symbols =
        super::optional_bounded_usize_input(name, input, "max_symbols", max_symbols)?
            .unwrap_or(max_symbols);
    let repo = super::canonicalize_tool_path(name, "repo_dir", repo_dir)?;
    let paths = super::repo_tool_paths(name, input)?;
    let glob = input.get("glob").and_then(Value::as_str);
    if let Some(glob) = glob {
        super::validate_git_pathspec(name, glob)?;
    }

    let symbols = repo_symbols_rg(
        name,
        &repo,
        &paths,
        glob,
        &name_filters,
        mode,
        &query,
        &fixed_terms,
        &smart_terms,
        effective_max_symbols,
    )?;

    let rendered = symbols
        .iter()
        .map(|symbol| {
            format!(
                "{}:{}:{} {} {}",
                symbol["path"].as_str().unwrap_or_default(),
                symbol["line"].as_u64().unwrap_or_default(),
                symbol["column"].as_u64().unwrap_or_default(),
                symbol["kind"].as_str().unwrap_or_default(),
                symbol["name"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let (content, truncated_bytes, bytes) =
        super::bytes_to_limited_text(rendered.as_bytes(), max_bytes);
    let truncated = symbols.len() >= effective_max_symbols || truncated_bytes;
    Ok(json!({
        "repo": repo.display().to_string(),
        "query": raw_query,
        "names": names,
        "effective_query": effective_query,
        "mode": mode,
        "symbols": symbols,
        "bytes": bytes,
        "truncated": truncated,
        "artifacts": [{
            "id": format!("repo-symbols:{}:{}", repo.display(), query),
            "kind": "repo_symbols",
            "title": "repo symbols",
            "uri": repo.display().to_string(),
            "content": content,
            "metadata": {
                "provider": "repo_symbols",
                "repo": repo.display().to_string(),
                "query": raw_query,
                "names": names,
                "effective_query": effective_query,
                "mode": mode,
                "paths": paths,
                "max_symbols": effective_max_symbols,
                "bytes": bytes,
                "truncated": truncated
            }
        }]
    }))
}

#[allow(clippy::too_many_arguments)]
fn repo_symbols_rg(
    name: &str,
    repo: &Path,
    paths: &[String],
    glob: Option<&str>,
    name_filters: &BTreeSet<String>,
    mode: &str,
    query: &str,
    fixed_terms: &[String],
    smart_terms: &[String],
    max_symbols: usize,
) -> Result<Vec<Value>, RuntimeError> {
    let pattern = r"^\s*(pub\s+|pub\([^)]*\)\s+|export\s+|async\s+|static\s+|final\s+|private\s+|protected\s+|public\s+|unsafe\s+)*(impl(\s+|<)|fn\s+|function\s+|def\s+|class\s+|struct\s+|enum\s+|trait\s+|interface\s+|type\s+)";
    let mut command = Command::new("rg");
    command.args([
        "--line-number",
        "--column",
        "--with-filename",
        "--no-heading",
        "--color",
        "never",
        pattern,
    ]);
    if let Some(glob) = glob {
        command.arg("-g").arg(glob);
    }
    if !paths.is_empty() {
        command.args(paths);
    }
    let output = command
        .current_dir(repo)
        .output()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} repo symbols: {error}")))?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(RuntimeError::Provider(format!(
            "tool {name} repo symbols failed: {}",
            super::provider_error_snippet(&String::from_utf8_lossy(&output.stderr))
        )));
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let symbol_lines_by_path = repo_symbol_lines_by_path(&raw);
    let mut total_lines_by_path = BTreeMap::new();
    let mut symbols = Vec::new();
    for line in raw.lines() {
        let item = super::parse_rg_vimgrep_line(line);
        let text = item["text"].as_str().unwrap_or_default();
        let Some((kind, symbol_name)) = parse_symbol_declaration(text) else {
            continue;
        };
        if !name_filters.is_empty() {
            if !name_filters.contains(&symbol_name.to_ascii_lowercase()) {
                continue;
            }
        } else if mode == "smart" {
            if !smart_terms.is_empty()
                && !repo_symbol_matches_any_term(&item, &symbol_name, text, smart_terms)
            {
                continue;
            }
        } else if !fixed_terms.is_empty() {
            if !repo_symbol_matches_any_term(&item, &symbol_name, text, fixed_terms) {
                continue;
            }
        } else if !query.is_empty()
            && !symbol_name.to_lowercase().contains(query)
            && !item["path"]
                .as_str()
                .unwrap_or_default()
                .to_lowercase()
                .contains(query)
        {
            continue;
        }
        let path = item["path"].as_str().unwrap_or_default();
        let line = item["line"].as_u64().unwrap_or_default();
        let end_line = repo_symbol_end_line(
            name,
            repo,
            path,
            line,
            text,
            &symbol_lines_by_path,
            &mut total_lines_by_path,
        )?;
        symbols.push(json!({
            "path": item["path"],
            "line": item["line"],
            "end_line": end_line,
            "column": item["column"],
            "kind": kind,
            "name": symbol_name,
            "text": text.trim()
        }));
        if symbols.len() >= max_symbols {
            break;
        }
    }
    Ok(symbols)
}

fn fixed_symbol_query_terms(query: &str) -> Vec<String> {
    let terms = query
        .split(|character: char| character.is_whitespace() || character == ',' || character == '|')
        .map(str::trim)
        .filter(|term| term.len() >= 3)
        .map(str::to_ascii_lowercase)
        .collect::<BTreeSet<_>>();
    if terms.len() <= 1 {
        Vec::new()
    } else {
        terms.into_iter().collect()
    }
}

fn repo_symbol_lines_by_path(raw: &str) -> BTreeMap<String, Vec<u64>> {
    let mut lines_by_path: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for line in raw.lines() {
        let item = super::parse_rg_vimgrep_line(line);
        let text = item["text"].as_str().unwrap_or_default();
        if parse_symbol_declaration(text).is_none() {
            continue;
        }
        let path = item["path"].as_str().unwrap_or_default();
        let line = item["line"].as_u64().unwrap_or_default();
        if path.is_empty() || line == 0 {
            continue;
        }
        lines_by_path
            .entry(path.to_string())
            .or_default()
            .push(line);
    }
    for lines in lines_by_path.values_mut() {
        lines.sort_unstable();
        lines.dedup();
    }
    lines_by_path
}

fn repo_symbol_end_line(
    tool_name: &str,
    repo: &Path,
    path: &str,
    start_line: u64,
    declaration_text: &str,
    symbol_lines_by_path: &BTreeMap<String, Vec<u64>>,
    total_lines_by_path: &mut BTreeMap<String, u64>,
) -> Result<u64, RuntimeError> {
    if is_rust_brace_block_declaration(path, declaration_text) {
        let absolute =
            super::canonicalize_tool_path(tool_name, "repo.symbols path", &repo.join(path))?;
        if absolute.starts_with(repo) && absolute.is_file() {
            let content = fs::read_to_string(&absolute).map_err(|error| {
                RuntimeError::Provider(format!("tool {tool_name} repo symbols: {error}"))
            })?;
            if let Some(end_line) = rust_brace_block_end_line(&content, start_line) {
                return Ok(end_line.max(start_line));
            }
        }
    }
    let Some(lines) = symbol_lines_by_path.get(path) else {
        return Ok(start_line);
    };
    if let Some(next_line) = lines.iter().copied().find(|line| *line > start_line) {
        return Ok(next_line.saturating_sub(1).max(start_line));
    }
    if let Some(total_lines) = total_lines_by_path.get(path) {
        return Ok((*total_lines).max(start_line));
    }
    let absolute = super::canonicalize_tool_path(tool_name, "repo.symbols path", &repo.join(path))?;
    if !absolute.starts_with(repo) || !absolute.is_file() {
        return Ok(start_line);
    }
    let content = fs::read_to_string(&absolute).map_err(|error| {
        RuntimeError::Provider(format!("tool {tool_name} repo symbols: {error}"))
    })?;
    let total_lines = content.lines().count().max(1) as u64;
    total_lines_by_path.insert(path.to_string(), total_lines);
    Ok(total_lines.max(start_line))
}

fn is_rust_brace_block_declaration(path: &str, declaration_text: &str) -> bool {
    if !path.ends_with(".rs") {
        return false;
    }
    let trimmed = declaration_text.trim_start();
    matches!(
        parse_symbol_declaration(trimmed).map(|(kind, _)| kind),
        Some("impl" | "function" | "struct" | "enum" | "interface")
    )
}

fn rust_brace_block_end_line(content: &str, start_line: u64) -> Option<u64> {
    let start_index = start_line.checked_sub(1)? as usize;
    let mut depth = 0usize;
    let mut saw_open = false;
    for (offset, line) in content.lines().enumerate().skip(start_index) {
        for character in line.chars() {
            match character {
                '{' => {
                    depth += 1;
                    saw_open = true;
                }
                '}' if saw_open => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(offset as u64 + 1);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn repo_symbol_matches_any_term(
    item: &Value,
    symbol_name: &str,
    declaration: &str,
    terms: &[String],
) -> bool {
    let path = item["path"].as_str().unwrap_or_default().to_lowercase();
    let name = symbol_name.to_lowercase();
    let text = declaration.to_lowercase();
    terms
        .iter()
        .any(|term| path.contains(term) || name.contains(term) || text.contains(term))
}

fn repo_symbol_names_input(tool_name: &str, input: &Value) -> Result<Vec<String>, RuntimeError> {
    let Some(raw_names) = input.get("names") else {
        return Ok(Vec::new());
    };
    let raw_names = raw_names.as_array().ok_or_else(|| {
        RuntimeError::Provider(format!("tool {tool_name} input.names must be an array"))
    })?;
    let mut names = Vec::new();
    for name in raw_names {
        let name = name.as_str().ok_or_else(|| {
            RuntimeError::Provider(format!(
                "tool {tool_name} input.names entries must be strings"
            ))
        })?;
        if name.trim().is_empty() {
            continue;
        }
        if !super::is_identifier_like(name) {
            return Err(RuntimeError::Provider(format!(
                "tool {tool_name} input.names entries must be identifier-like tokens"
            )));
        }
        names.push(name.to_string());
    }
    names.sort();
    names.dedup();
    Ok(names)
}

pub(super) fn parse_symbol_declaration(line: &str) -> Option<(&'static str, String)> {
    let trimmed = line.trim_start();
    if let Some(name) = parse_rust_impl_symbol_name(trimmed) {
        return Some(("impl", name));
    }
    let mut tokens = line
        .trim_start()
        .split(|character: char| character.is_whitespace() || character == '(' || character == '<')
        .filter(|token| !token.is_empty());
    let mut token = tokens.next()?;
    while matches!(
        token,
        "pub"
            | "crate)"
            | "self)"
            | "super)"
            | "export"
            | "async"
            | "unsafe"
            | "const"
            | "static"
            | "final"
            | "private"
            | "protected"
            | "public"
    ) || token.starts_with("pub(")
    {
        token = tokens.next()?;
    }
    let kind = match token {
        "fn" | "function" | "def" => "function",
        "class" => "class",
        "struct" => "struct",
        "enum" => "enum",
        "trait" | "interface" => "interface",
        "type" => "type",
        "const" | "let" | "var" => "variable",
        _ => return None,
    };
    let raw_name = tokens.next()?;
    let name = raw_name
        .trim_matches(|character: char| {
            !(character.is_ascii_alphanumeric() || character == '_' || character == '$')
        })
        .to_string();
    (!name.is_empty()).then_some((kind, name))
}

fn parse_rust_impl_symbol_name(line: &str) -> Option<String> {
    let mut text = line;
    for modifier in ["pub ", "unsafe "] {
        if let Some(rest) = text.strip_prefix(modifier) {
            text = rest.trim_start();
        }
    }
    let rest = text.strip_prefix("impl")?.trim_start();
    if rest.is_empty() {
        return None;
    }
    let rest = strip_rust_generics_prefix(rest).trim_start();
    let target = if let Some((_, target)) = rest.rsplit_once(" for ") {
        target
    } else {
        rest
    };
    rust_type_symbol_name(target)
}

fn strip_rust_generics_prefix(text: &str) -> &str {
    let Some(rest) = text.strip_prefix('<') else {
        return text;
    };
    let mut depth = 1usize;
    for (index, character) in rest.char_indices() {
        match character {
            '<' => depth += 1,
            '>' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return &rest[index + character.len_utf8()..];
                }
            }
            _ => {}
        }
    }
    text
}

fn rust_type_symbol_name(text: &str) -> Option<String> {
    let cleaned = text
        .trim_start()
        .trim_start_matches('&')
        .trim_start_matches("mut ")
        .trim_start();
    let token = cleaned
        .split(|character: char| {
            character.is_whitespace()
                || character == '<'
                || character == '{'
                || character == '('
                || character == ';'
        })
        .next()?;
    let segment = token.rsplit("::").next().unwrap_or(token);
    let name = segment
        .trim_matches(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .to_string();
    (!name.is_empty()).then_some(name)
}
