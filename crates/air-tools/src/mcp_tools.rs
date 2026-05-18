use super::*;
use air_tools_core::text::bytes_to_limited_text;

const DEFAULT_MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const DEFAULT_MCP_CLIENT_NAME: &str = "air-tools";
const DEFAULT_MCP_CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub(super) struct McpToolConfig<'a> {
    pub server_url: &'a str,
    pub tool: Option<&'a str>,
    pub headers: &'a BTreeMap<String, String>,
    pub bearer_token_env: Option<&'a str>,
    pub timeout_seconds: Option<u64>,
    pub action_timeout: Option<Duration>,
    pub protocol_version: Option<&'a str>,
    pub client_name: Option<&'a str>,
    pub client_version: Option<&'a str>,
    pub max_bytes: usize,
}

struct McpHttpResponse {
    body: String,
    session_id: Option<String>,
}

pub(super) fn call_mcp_tool(
    name: &str,
    input: &Value,
    config: McpToolConfig<'_>,
) -> Result<Value, RuntimeError> {
    let server_url = render_json_template(config.server_url, input);
    if !(server_url.starts_with("http://") || server_url.starts_with("https://")) {
        return Err(RuntimeError::Provider(format!(
            "tool {name} tools.{name}.server_url must start with http:// or https://"
        )));
    }
    let request_timeout = compute_request_timeout(config.timeout_seconds, config.action_timeout);
    let client = reqwest::blocking::Client::builder()
        .timeout(request_timeout)
        .build()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} MCP client: {error}")))?;

    let initialize = mcp_initialize(name, &client, &server_url, input, &config)?;
    mcp_initialized_notification(name, &client, &server_url, input, &config, &initialize)?;

    let requested_tool = input
        .get("tool")
        .or_else(|| input.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let action = input
        .get("action")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let mcp_tool = config.tool.or(requested_tool);
    if action == "list_tools" || mcp_tool.is_none() {
        let tools = mcp_request_result(
            name,
            &client,
            &server_url,
            input,
            &config,
            initialize.session_id.as_deref(),
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list",
                "params": {}
            }),
        )?;
        return Ok(json!({
            "kind": "mcp_tools",
            "server_url": server_url,
            "initialize": initialize_value(&initialize.body, config.max_bytes),
            "tools": limit_json_output(tools, config.max_bytes),
        }));
    }

    let mcp_tool = mcp_tool.expect("checked above");
    let arguments = mcp_tool_arguments(input);
    let result = mcp_request_result(
        name,
        &client,
        &server_url,
        input,
        &config,
        initialize.session_id.as_deref(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": mcp_tool,
                "arguments": arguments
            }
        }),
    )?;
    Ok(json!({
        "kind": "mcp_tool_result",
        "server_url": server_url,
        "tool": mcp_tool,
        "result": limit_json_output(result, config.max_bytes),
    }))
}

fn compute_request_timeout(
    timeout_seconds: Option<u64>,
    action_timeout: Option<Duration>,
) -> Duration {
    let configured_timeout = Duration::from_secs(timeout_seconds.unwrap_or(30));
    action_timeout
        .map(|timeout| timeout.min(configured_timeout))
        .unwrap_or(configured_timeout)
}

fn mcp_initialize(
    tool_name: &str,
    client: &reqwest::blocking::Client,
    server_url: &str,
    input: &Value,
    config: &McpToolConfig<'_>,
) -> Result<McpHttpResponse, RuntimeError> {
    mcp_post(
        tool_name,
        client,
        server_url,
        input,
        config,
        None,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": config.protocol_version.unwrap_or(DEFAULT_MCP_PROTOCOL_VERSION),
                "capabilities": {
                    "roots": {
                        "listChanged": false
                    },
                    "sampling": {}
                },
                "clientInfo": {
                    "name": config.client_name.unwrap_or(DEFAULT_MCP_CLIENT_NAME),
                    "version": config.client_version.unwrap_or(DEFAULT_MCP_CLIENT_VERSION)
                }
            }
        }),
    )
}

fn mcp_initialized_notification(
    tool_name: &str,
    client: &reqwest::blocking::Client,
    server_url: &str,
    input: &Value,
    config: &McpToolConfig<'_>,
    initialize: &McpHttpResponse,
) -> Result<(), RuntimeError> {
    let _ = mcp_post(
        tool_name,
        client,
        server_url,
        input,
        config,
        initialize.session_id.as_deref(),
        &json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }),
    )?;
    Ok(())
}

fn mcp_request_result(
    tool_name: &str,
    client: &reqwest::blocking::Client,
    server_url: &str,
    input: &Value,
    config: &McpToolConfig<'_>,
    session_id: Option<&str>,
    body: Value,
) -> Result<Value, RuntimeError> {
    let response = mcp_post(
        tool_name, client, server_url, input, config, session_id, &body,
    )?;
    let value = parse_mcp_json(tool_name, &response.body)?;
    if let Some(error) = value.get("error") {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} MCP error: {}",
            provider_error_snippet(&error.to_string())
        )));
    }
    value.get("result").cloned().ok_or_else(|| {
        RuntimeError::Provider(format!(
            "tool {tool_name} MCP response missing result: {}",
            provider_error_snippet(&response.body)
        ))
    })
}

fn mcp_post(
    tool_name: &str,
    client: &reqwest::blocking::Client,
    server_url: &str,
    input: &Value,
    config: &McpToolConfig<'_>,
    session_id: Option<&str>,
    body: &Value,
) -> Result<McpHttpResponse, RuntimeError> {
    let mut request = client
        .post(server_url)
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .header(reqwest::header::CONTENT_TYPE, "application/json");
    for (header, value) in config.headers {
        request = request.header(header, render_json_template(value, input));
    }
    if let Some(env_name) = config.bearer_token_env {
        let token = std::env::var(env_name).map_err(|_| {
            RuntimeError::Provider(format!(
                "tool {tool_name} requires bearer_token_env {env_name}, but it is not set"
            ))
        })?;
        request = request.bearer_auth(token);
    }
    if let Some(session_id) = session_id {
        request = request.header("Mcp-Session-Id", session_id);
    }

    let response = request.json(body).send().map_err(|error| {
        RuntimeError::Provider(format!("tool {tool_name} MCP request: {error}"))
    })?;
    let status = response.status();
    let session_id = response
        .headers()
        .get("Mcp-Session-Id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = response.text().map_err(|error| {
        RuntimeError::Provider(format!("tool {tool_name} MCP response: {error}"))
    })?;
    if !status.is_success() {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} MCP HTTP status {}: {}",
            status.as_u16(),
            provider_error_snippet(&body)
        )));
    }
    Ok(McpHttpResponse { body, session_id })
}

fn parse_mcp_json(tool_name: &str, body: &str) -> Result<Value, RuntimeError> {
    if let Ok(value) = serde_json::from_str(body) {
        return Ok(value);
    }
    if let Some(value) = parse_sse_json(body) {
        return Ok(value);
    }
    Err(RuntimeError::Provider(format!(
        "tool {tool_name} MCP response was not valid JSON: {}",
        provider_error_snippet(body)
    )))
}

fn parse_sse_json(body: &str) -> Option<Value> {
    body.lines()
        .filter_map(|line| line.trim().strip_prefix("data:"))
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != "[DONE]")
        .filter_map(|line| serde_json::from_str(line).ok())
        .last()
}

fn mcp_tool_arguments(input: &Value) -> Value {
    if let Some(arguments) = input.get("arguments").and_then(Value::as_object) {
        return Value::Object(arguments.clone());
    }
    if let Some(arguments) = input.get("params").and_then(Value::as_object) {
        return Value::Object(arguments.clone());
    }
    let Some(object) = input.as_object() else {
        return json!({});
    };
    let arguments = object
        .iter()
        .filter(|(key, _)| {
            !matches!(
                key.as_str(),
                "action" | "tool" | "name" | "params" | "arguments"
            )
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    Value::Object(arguments)
}

fn initialize_value(body: &str, max_bytes: usize) -> Value {
    parse_sse_json(body)
        .or_else(|| serde_json::from_str(body).ok())
        .map(|value| value.get("result").cloned().unwrap_or(value))
        .map(|value| limit_json_output(value, max_bytes))
        .unwrap_or_else(|| json!({}))
}

fn limit_json_output(value: Value, max_bytes: usize) -> Value {
    let Ok(bytes) = serde_json::to_vec(&value) else {
        return value;
    };
    if bytes.len() <= max_bytes {
        return value;
    }
    let (preview, truncated, bytes_returned) = bytes_to_limited_text(&bytes, max_bytes);
    json!({
        "truncated": truncated,
        "bytes": bytes.len(),
        "bytes_returned": bytes_returned,
        "preview": preview,
    })
}
