use air_runtime::ToolProvider;
use air_tools::ConfigTools;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

pub(crate) struct McpListOptions {
    pub(crate) tool_config: PathBuf,
}

pub(crate) struct McpExplainOptions {
    pub(crate) tool: String,
    pub(crate) tool_config: PathBuf,
}

pub(crate) struct McpAuditOptions {
    pub(crate) tool: Option<String>,
    pub(crate) tool_config: PathBuf,
}

pub(crate) struct McpCallOptions {
    pub(crate) tool: String,
    pub(crate) tool_config: PathBuf,
    pub(crate) input_json: Option<String>,
}

#[derive(Debug, Clone)]
struct McpEntry {
    name: String,
    transport: String,
    server_url: Option<String>,
    command: Option<String>,
    args: Vec<String>,
    env_keys: Vec<String>,
    cwd: Option<String>,
    tool: Option<String>,
    capability: Option<String>,
    bearer_token_env: Option<String>,
    header_count: usize,
    timeout_seconds: Option<u64>,
}

pub(crate) fn list_mcp(options: McpListOptions) -> Result<()> {
    let entries = read_mcp_entries(&options.tool_config)?;
    print_json(&json!({
        "tool_config": options.tool_config,
        "mcp_tools": entries.iter().map(mcp_entry_json).collect::<Vec<_>>(),
    }))
}

pub(crate) fn explain_mcp(options: McpExplainOptions) -> Result<()> {
    let entry = find_mcp_entry(&options.tool_config, &options.tool)?;
    print_json(&json!({
        "tool_config": options.tool_config,
        "mcp": mcp_entry_json(&entry),
        "permissions": {
            "capability": entry.capability.clone().unwrap_or_else(|| "net.mcp".to_string()),
            "network": "required",
            "auth": if entry.bearer_token_env.is_some() || entry.header_count > 0 {
                "configured"
            } else {
                "not configured"
            },
            "tool_call": if entry.tool.is_some() {
                "fixed MCP tool"
            } else {
                "model/request selects MCP tool"
            },
        },
        "run_behavior": [
            "initialize MCP session",
            "send notifications/initialized",
            "list tools when no fixed tool/action is supplied",
            "call tools/call for fixed or requested MCP tools"
        ],
    }))
}

pub(crate) fn audit_mcp(options: McpAuditOptions) -> Result<()> {
    let entries = if let Some(tool) = &options.tool {
        vec![find_mcp_entry(&options.tool_config, tool)?]
    } else {
        read_mcp_entries(&options.tool_config)?
    };
    let audited = entries
        .iter()
        .map(|entry| {
            let findings = mcp_findings(entry);
            let risk = mcp_risk(&findings);
            json!({
                "tool": entry.name,
                "server_url": entry.server_url,
                "capability": entry.capability,
                "risk": risk,
                "findings": findings,
            })
        })
        .collect::<Vec<_>>();
    print_json(&json!({
        "tool_config": options.tool_config,
        "mcp_audit": audited,
    }))
}

pub(crate) fn call_mcp(options: McpCallOptions) -> Result<()> {
    let input = options
        .input_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .context("parse --input-json")?
        .unwrap_or_else(|| json!({ "action": "list_tools" }));
    let mut tools = ConfigTools::from_file(options.tool_config.clone())
        .with_context(|| format!("load {}", options.tool_config.display()))?;
    let output = tools.call_tool(&options.tool, &input)?;
    print_json(&json!({
        "tool_config": options.tool_config,
        "tool": options.tool,
        "input": input,
        "output": output
    }))
}

fn find_mcp_entry(tool_config: &PathBuf, name: &str) -> Result<McpEntry> {
    read_mcp_entries(tool_config)?
        .into_iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| anyhow::anyhow!("MCP tool `{name}` not found in {}", tool_config.display()))
}

fn read_mcp_entries(path: &PathBuf) -> Result<Vec<McpEntry>> {
    let value: Value = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("read {}", path.display()))?,
    )
    .with_context(|| format!("parse {}", path.display()))?;
    let Some(tools) = value.get("tools").and_then(Value::as_object) else {
        bail!(
            "tool config {} missing object field `tools`",
            path.display()
        );
    };
    let mut entries = Vec::new();
    for (name, config) in tools {
        if config.get("kind").and_then(Value::as_str) != Some("mcp") {
            continue;
        }
        let server_url = config
            .get("server_url")
            .and_then(Value::as_str)
            .map(str::to_string);
        let command = config
            .get("command")
            .and_then(Value::as_str)
            .map(str::to_string);
        let transport = match (&server_url, &command) {
            (Some(_), None) => "http",
            (None, Some(_)) => "stdio",
            (Some(_), Some(_)) => "invalid",
            (None, None) => "invalid",
        }
        .to_string();
        let args = config
            .get("args")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let env_keys = config
            .get("env")
            .and_then(Value::as_object)
            .map(|env| env.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        entries.push(McpEntry {
            name: name.clone(),
            transport,
            server_url,
            command,
            args,
            env_keys,
            cwd: config
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::to_string),
            tool: config
                .get("tool")
                .and_then(Value::as_str)
                .map(str::to_string),
            capability: config
                .get("capability")
                .and_then(Value::as_str)
                .map(str::to_string),
            bearer_token_env: config
                .get("bearer_token_env")
                .and_then(Value::as_str)
                .map(str::to_string),
            header_count: config
                .get("headers")
                .and_then(Value::as_object)
                .map_or(0, |headers| headers.len()),
            timeout_seconds: config.get("timeout_seconds").and_then(Value::as_u64),
        });
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(entries)
}

fn mcp_entry_json(entry: &McpEntry) -> Value {
    json!({
        "name": entry.name,
        "kind": "mcp",
        "transport": entry.transport,
        "server_url": entry.server_url,
        "command": entry.command,
        "args": entry.args,
        "env_keys": entry.env_keys,
        "cwd": entry.cwd,
        "tool": entry.tool,
        "capability": entry.capability,
        "bearer_token_env": entry.bearer_token_env,
        "header_count": entry.header_count,
        "timeout_seconds": entry.timeout_seconds,
    })
}

fn mcp_findings(entry: &McpEntry) -> Vec<Value> {
    let mut findings = Vec::new();
    if entry.transport == "invalid" {
        findings.push(json!({
            "severity": "critical",
            "reason": "MCP transport is invalid; set exactly one of server_url or command",
        }));
    }
    if entry
        .server_url
        .as_deref()
        .is_some_and(|url| url.starts_with("http://"))
    {
        findings.push(json!({
            "severity": "high",
            "reason": "MCP server uses plaintext HTTP",
        }));
    }
    if entry
        .server_url
        .as_deref()
        .is_some_and(|url| url.contains('{') || url.contains("{{"))
    {
        findings.push(json!({
            "severity": "medium",
            "reason": "MCP server URL is templated from model/tool input",
        }));
    }
    if entry.transport == "stdio" {
        findings.push(json!({
            "severity": "medium",
            "reason": "MCP stdio transport executes a local command",
        }));
        if entry.command.as_deref() == Some("npx") {
            findings.push(json!({
                "severity": "medium",
                "reason": "MCP stdio command downloads/runs an npm package through npx",
            }));
        }
    }
    if entry.bearer_token_env.is_none() && entry.header_count == 0 && entry.env_keys.is_empty() {
        findings.push(json!({
            "severity": "medium",
            "reason": "MCP server has no configured authentication headers",
        }));
    }
    if entry.tool.is_none() {
        findings.push(json!({
            "severity": "low",
            "reason": "MCP tool name is selected at runtime",
        }));
    }
    if entry.capability.as_deref().unwrap_or("net.mcp") == "net.mcp" {
        findings.push(json!({
            "severity": "low",
            "reason": "MCP capability is generic; prefer a narrow capability such as github.issue.read",
        }));
    }
    findings
}

fn mcp_risk(findings: &[Value]) -> &'static str {
    if findings
        .iter()
        .any(|finding| finding.get("severity").and_then(Value::as_str) == Some("critical"))
    {
        "critical"
    } else if findings
        .iter()
        .any(|finding| finding.get("severity").and_then(Value::as_str) == Some("high"))
    {
        "high"
    } else if findings
        .iter()
        .any(|finding| finding.get("severity").and_then(Value::as_str) == Some("medium"))
    {
        "medium"
    } else {
        "low"
    }
}

fn print_json(value: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
