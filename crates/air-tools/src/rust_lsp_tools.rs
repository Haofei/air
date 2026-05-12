use air_runtime::RuntimeError;
use regex::Regex;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

#[derive(Debug)]
pub(crate) struct RustAnalyzerSession {
    root: PathBuf,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    child: Child,
    next_id: u64,
}

impl RustAnalyzerSession {
    pub(crate) fn start(name: &str, root_dir: &Path, command: &str) -> Result<Self, RuntimeError> {
        let root = super::canonicalize_tool_path(name, "root_dir", root_dir)?;
        let root_uri = file_uri(&root);
        let mut child = Command::new(command)
            .arg("--log-file")
            .arg("/dev/null")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                RuntimeError::Provider(format!("tool {name} start {command}: {error}"))
            })?;
        let mut stdin = child.stdin.take().ok_or_else(|| {
            RuntimeError::Provider(format!("tool {name} could not open rust-analyzer stdin"))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            RuntimeError::Provider(format!("tool {name} could not open rust-analyzer stdout"))
        })?;
        let mut reader = BufReader::new(stdout);

        send_lsp_message(
            &mut stdin,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": root_uri,
                    "capabilities": {}
                }
            }),
        )?;
        let initialize = read_lsp_response(name, &mut reader, 1)?;
        if initialize.get("error").is_some() {
            let _ = child.kill();
            return Err(RuntimeError::Provider(format!(
                "tool {name} rust-analyzer initialize failed: {}",
                compact_json(&initialize["error"])
            )));
        }
        send_lsp_message(
            &mut stdin,
            &json!({
                "jsonrpc": "2.0",
                "method": "initialized",
                "params": {}
            }),
        )?;

        Ok(Self {
            root,
            stdin,
            reader,
            child,
            next_id: 2,
        })
    }

    fn next_request_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub(crate) fn shutdown(&mut self) {
        let shutdown_id = self.next_request_id();
        let _ = send_lsp_message(
            &mut self.stdin,
            &json!({
                "jsonrpc": "2.0",
                "id": shutdown_id,
                "method": "shutdown",
                "params": null
            }),
        );
        let _ = send_lsp_message(
            &mut self.stdin,
            &json!({
                "jsonrpc": "2.0",
                "method": "exit"
            }),
        );
        let _ = self.child.kill();
    }

    fn references(
        &mut self,
        name: &str,
        file_uri: &str,
        position: &LspPosition,
        include_declaration: bool,
    ) -> Result<Value, RuntimeError> {
        let mut response = json!({"result": []});
        for _ in 0..20 {
            let request_id = self.next_request_id();
            send_lsp_message(
                &mut self.stdin,
                &json!({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "method": "textDocument/references",
                    "params": {
                        "textDocument": {"uri": file_uri},
                        "position": {
                            "line": position.line_zero_based,
                            "character": position.character_zero_based
                        },
                        "context": {"includeDeclaration": include_declaration}
                    }
                }),
            )?;
            response = read_lsp_response(name, &mut self.reader, request_id)?;
            if response.get("error").is_some() && !is_retryable_lsp_error(&response) {
                break;
            }
            if response
                .get("result")
                .and_then(Value::as_array)
                .is_some_and(|locations| !locations.is_empty())
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        Ok(response)
    }
}

impl Drop for RustAnalyzerSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub(crate) fn call_lsp_references_tool(
    name: &str,
    session: &mut RustAnalyzerSession,
    input: &Value,
    max_results: usize,
    max_bytes: usize,
) -> Result<Value, RuntimeError> {
    let path = super::required_input_string(name, input, "path")?;
    super::validate_git_pathspec(name, path)?;
    let symbol = super::optional_string_input(name, input, "symbol")?;
    let include_declaration =
        super::optional_bool_input(name, input, "include_declaration")?.unwrap_or(true);
    let effective_max_results =
        super::optional_bounded_usize_input(name, input, "max_results", max_results)?
            .unwrap_or(max_results);

    let root = session.root.clone();
    let file = super::canonicalize_tool_path(name, "path", &root.join(path))?;
    if !file.starts_with(&root) {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.path is outside root_dir"
        )));
    }
    let source = fs::read_to_string(&file)
        .map_err(|error| RuntimeError::Provider(format!("tool {name} read path: {error}")))?;
    let position = lsp_position_input(name, input, symbol, &source)?;
    let file_uri = file_uri(&file);
    let response = session.references(name, &file_uri, &position, include_declaration)?;

    if response.get("error").is_some() {
        return Err(RuntimeError::Provider(format!(
            "tool {name} rust-analyzer references failed: {}",
            compact_json(&response["error"])
        )));
    }
    let raw_locations = response
        .get("result")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut references = Vec::new();
    for location in raw_locations.into_iter().take(effective_max_results) {
        if let Some(reference) = lsp_location_to_value(&root, &location) {
            references.push(reference);
        }
    }
    let truncated = references.len() >= effective_max_results;
    let rendered = serde_json::to_string_pretty(&references).unwrap_or_else(|_| "[]".to_string());
    let (content, truncated_bytes, bytes) =
        super::bytes_to_limited_text(rendered.as_bytes(), max_bytes);
    Ok(json!({
        "repo": root.display().to_string(),
        "path": path,
        "symbol": position.symbol,
        "line": position.line_zero_based + 1,
        "character": position.character_zero_based,
        "references": references,
        "reference_count": references.len(),
        "truncated": truncated || truncated_bytes,
        "bytes": bytes,
        "artifacts": [{
            "id": format!("lsp-references:{}:{}:{}", root.display(), path, position.symbol.clone().unwrap_or_default()),
            "kind": "lsp_references",
            "title": "rust-analyzer references",
            "uri": root.display().to_string(),
            "content": content,
            "metadata": {
                "provider": "rust_analyzer",
                "tool": name,
                "path": path,
                "symbol": position.symbol,
                "reference_count": references.len(),
                "truncated": truncated || truncated_bytes
            }
        }]
    }))
}

pub(crate) fn call_lsp_diagnostics_tool(
    name: &str,
    input: &Value,
    root_dir: &Path,
    command: &str,
    max_diagnostics: usize,
    max_bytes: usize,
) -> Result<Value, RuntimeError> {
    let path_filter = super::optional_string_input(name, input, "path")?;
    if let Some(path) = path_filter {
        super::validate_git_pathspec(name, path)?;
    }
    let effective_max_diagnostics =
        super::optional_bounded_usize_input(name, input, "max_diagnostics", max_diagnostics)?
            .unwrap_or(max_diagnostics);
    let root = super::canonicalize_tool_path(name, "root_dir", root_dir)?;
    let output = Command::new(command)
        .arg("diagnostics")
        .arg(&root)
        .arg("--disable-build-scripts")
        .arg("--disable-proc-macros")
        .output()
        .map_err(|error| {
            RuntimeError::Provider(format!("tool {name} run {command} diagnostics: {error}"))
        })?;
    let raw = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let diagnostics = parse_rust_analyzer_diagnostics(&raw, &root, path_filter)
        .into_iter()
        .take(effective_max_diagnostics)
        .collect::<Vec<_>>();
    let rendered = serde_json::to_string_pretty(&diagnostics).unwrap_or_else(|_| "[]".to_string());
    let (content, truncated_bytes, bytes) =
        super::bytes_to_limited_text(rendered.as_bytes(), max_bytes);
    Ok(json!({
        "repo": root.display().to_string(),
        "success": output.status.success(),
        "status": output.status.code(),
        "diagnostics": diagnostics,
        "diagnostic_count": diagnostics.len(),
        "raw_truncated": truncated_bytes,
        "bytes": bytes,
        "artifacts": [{
            "id": format!("lsp-diagnostics:{}", root.display()),
            "kind": "lsp_diagnostics",
            "title": "rust-analyzer diagnostics",
            "uri": root.display().to_string(),
            "content": content,
            "metadata": {
                "provider": "rust_analyzer",
                "tool": name,
                "diagnostic_count": diagnostics.len(),
                "status": output.status.code()
            }
        }]
    }))
}

#[derive(Debug)]
struct LspPosition {
    line_zero_based: usize,
    character_zero_based: usize,
    symbol: Option<String>,
}

fn lsp_position_input(
    tool_name: &str,
    input: &Value,
    symbol: Option<&str>,
    source: &str,
) -> Result<LspPosition, RuntimeError> {
    if let Some(line) = input.get("line").and_then(Value::as_u64) {
        let character = input
            .get("character")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        if line == 0 {
            return Err(RuntimeError::Provider(format!(
                "tool {tool_name} input.line is 1-based and must be positive"
            )));
        }
        return Ok(LspPosition {
            line_zero_based: (line - 1) as usize,
            character_zero_based: character as usize,
            symbol: symbol.map(ToString::to_string),
        });
    }
    let symbol = symbol.ok_or_else(|| {
        RuntimeError::Provider(format!(
            "tool {tool_name} requires either input.symbol or input.line"
        ))
    })?;
    find_symbol_position(source, symbol)
        .map(|(line_zero_based, character_zero_based)| LspPosition {
            line_zero_based,
            character_zero_based,
            symbol: Some(symbol.to_string()),
        })
        .ok_or_else(|| {
            RuntimeError::Provider(format!(
                "tool {tool_name} symbol {symbol} was not found in input.path"
            ))
        })
}

fn find_symbol_position(source: &str, symbol: &str) -> Option<(usize, usize)> {
    for (line_index, line) in source.lines().enumerate() {
        let mut search_start = 0usize;
        while let Some(offset) = line[search_start..].find(symbol) {
            let column = search_start + offset;
            let before = line[..column].chars().next_back();
            let after = line[column + symbol.len()..].chars().next();
            if !is_ident_char(before) && !is_ident_char(after) {
                return Some((line_index, column));
            }
            search_start = column + symbol.len();
        }
    }
    None
}

fn is_ident_char(character: Option<char>) -> bool {
    character.is_some_and(|value| value.is_ascii_alphanumeric() || value == '_')
}

fn send_lsp_message(writer: &mut impl Write, message: &Value) -> Result<(), RuntimeError> {
    let body = serde_json::to_vec(message)
        .map_err(|error| RuntimeError::Provider(format!("serialize LSP message: {error}")))?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())
        .and_then(|_| writer.write_all(&body))
        .and_then(|_| writer.flush())
        .map_err(|error| RuntimeError::Provider(format!("write LSP message: {error}")))
}

fn read_lsp_response(
    tool_name: &str,
    reader: &mut BufReader<impl Read>,
    wanted_id: u64,
) -> Result<Value, RuntimeError> {
    loop {
        let message = read_lsp_message(tool_name, reader)?;
        if message.get("id").and_then(Value::as_u64) == Some(wanted_id) {
            return Ok(message);
        }
    }
}

fn read_lsp_message(
    tool_name: &str,
    reader: &mut BufReader<impl Read>,
) -> Result<Value, RuntimeError> {
    let mut content_length = None;
    loop {
        let mut header = String::new();
        let bytes = reader.read_line(&mut header).map_err(|error| {
            RuntimeError::Provider(format!("tool {tool_name} read LSP header: {error}"))
        })?;
        if bytes == 0 {
            return Err(RuntimeError::Provider(format!(
                "tool {tool_name} rust-analyzer closed stdout"
            )));
        }
        let header = header.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse::<usize>().ok();
            }
        }
    }
    let length = content_length.ok_or_else(|| {
        RuntimeError::Provider(format!(
            "tool {tool_name} LSP response missing Content-Length"
        ))
    })?;
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).map_err(|error| {
        RuntimeError::Provider(format!("tool {tool_name} read LSP body: {error}"))
    })?;
    serde_json::from_slice(&body).map_err(|error| {
        RuntimeError::Provider(format!("tool {tool_name} parse LSP body: {error}"))
    })
}

fn lsp_location_to_value(root: &Path, location: &Value) -> Option<Value> {
    let uri = location.get("uri").and_then(Value::as_str)?;
    let range = location.get("range")?;
    let start = range.get("start")?;
    let end = range.get("end")?;
    let absolute = file_uri_to_path(uri);
    let path = absolute
        .as_ref()
        .and_then(|path| path.strip_prefix(root).ok())
        .and_then(|path| path.to_str())
        .unwrap_or(uri)
        .to_string();
    Some(json!({
        "path": path,
        "line": start.get("line").and_then(Value::as_u64).unwrap_or_default() + 1,
        "character": start.get("character").and_then(Value::as_u64).unwrap_or_default(),
        "end_line": end.get("line").and_then(Value::as_u64).unwrap_or_default() + 1,
        "end_character": end.get("character").and_then(Value::as_u64).unwrap_or_default(),
        "uri": uri
    }))
}

fn is_retryable_lsp_error(response: &Value) -> bool {
    let message = response
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    message.contains("file not found") || message.contains("content modified")
}

fn file_uri(path: &Path) -> String {
    format!("file://{}", percent_encode_path(&path.to_string_lossy()))
}

fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    Some(PathBuf::from(percent_decode(rest)))
}

fn percent_encode_path(path: &str) -> String {
    let mut encoded = String::new();
    for byte in path.as_bytes() {
        let character = *byte as char;
        if character.is_ascii_alphanumeric() || matches!(character, '/' | '-' | '_' | '.' | '~') {
            encoded.push(character);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(hex) = u8::from_str_radix(&value[index + 1..index + 3], 16) {
                decoded.push(hex);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).to_string()
}

fn parse_rust_analyzer_diagnostics(
    raw: &str,
    root: &Path,
    path_filter: Option<&str>,
) -> Vec<Value> {
    let cleaned = raw.replace('\x08', "");
    let pattern = Regex::new(
        r#"at crate (?P<crate>[^,]+), file (?P<file>[^:]+): (?P<severity>\w+) (?P<code>[^ ]+) from LineCol \{ line: (?P<line>\d+), col: (?P<col>\d+) \} to LineCol \{ line: (?P<end_line>\d+), col: (?P<end_col>\d+) \}: (?P<message>.+)"#,
    )
    .expect("valid rust-analyzer diagnostic regex");
    let mut diagnostics = Vec::new();
    for captures in pattern.captures_iter(&cleaned) {
        let file = captures
            .name("file")
            .map(|value| value.as_str())
            .unwrap_or_default();
        let path = Path::new(file)
            .strip_prefix(root)
            .ok()
            .and_then(|path| path.to_str())
            .unwrap_or(file)
            .to_string();
        if path_filter.is_some_and(|filter| filter != path) {
            continue;
        }
        diagnostics.push(json!({
            "crate": captures.name("crate").map(|value| value.as_str()).unwrap_or_default(),
            "path": path,
            "severity": captures.name("severity").map(|value| value.as_str()).unwrap_or_default(),
            "code": captures.name("code").map(|value| value.as_str()).unwrap_or_default(),
            "line": captures.name("line").and_then(|value| value.as_str().parse::<u64>().ok()).unwrap_or_default() + 1,
            "character": captures.name("col").and_then(|value| value.as_str().parse::<u64>().ok()).unwrap_or_default(),
            "end_line": captures.name("end_line").and_then(|value| value.as_str().parse::<u64>().ok()).unwrap_or_default() + 1,
            "end_character": captures.name("end_col").and_then(|value| value.as_str().parse::<u64>().ok()).unwrap_or_default(),
            "message": captures.name("message").map(|value| value.as_str().trim()).unwrap_or_default()
        }));
    }
    diagnostics
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "<unserializable>".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_symbol_position_on_token_boundary() {
        let source = "fn helper_alias() {}\nfn helper() {}\n";
        assert_eq!(find_symbol_position(source, "helper"), Some((1, 3)));
    }

    #[test]
    fn parses_rust_analyzer_diagnostic_lines() {
        let raw = r#"at crate air, file /repo/src/main.rs: Error RustcHardError("E0599") from LineCol { line: 4, col: 2 } to LineCol { line: 4, col: 8 }: no method named foo"#;
        let diagnostics = parse_rust_analyzer_diagnostics(raw, Path::new("/repo"), None);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0]["path"], json!("src/main.rs"));
        assert_eq!(diagnostics[0]["line"], json!(5));
        assert_eq!(diagnostics[0]["message"], json!("no method named foo"));
    }

    #[test]
    #[ignore = "requires rust-analyzer installed on PATH"]
    fn live_lsp_references_finds_temp_project_refs() {
        let root = std::env::temp_dir().join(format!("air-tools-live-lsp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"air_tools_live_lsp\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "pub fn helper() -> usize { 1 }\n\npub fn caller() -> usize { helper() }\n",
        )
        .unwrap();

        let mut session =
            RustAnalyzerSession::start("lsp.references", &root, "rust-analyzer").unwrap();
        let output = call_lsp_references_tool(
            "lsp.references",
            &mut session,
            &json!({
                "path": "src/lib.rs",
                "symbol": "helper",
                "include_declaration": true
            }),
            20,
            65536,
        )
        .unwrap();

        assert!(
            output["reference_count"].as_u64().unwrap_or_default() >= 2,
            "{output}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    #[ignore = "requires rust-analyzer installed on PATH"]
    fn live_lsp_diagnostics_reports_temp_project_errors() {
        let root = std::env::temp_dir().join(format!(
            "air-tools-live-lsp-diagnostics-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"air_tools_live_lsp_diagnostics\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "pub fn broken() { missing_symbol(); }\n",
        )
        .unwrap();

        let output = call_lsp_diagnostics_tool(
            "lsp.diagnostics",
            &json!({"path": "src/lib.rs"}),
            &root,
            "rust-analyzer",
            20,
            65536,
        )
        .unwrap();

        assert!(
            output["diagnostic_count"].as_u64().unwrap_or_default() >= 1,
            "{output}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
