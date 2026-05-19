use super::{
    canonicalize_tool_path, provider_error_snippet, render_json_template, required_input_string,
};
use air_runtime::RuntimeError;
use anyhow::Result;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub(super) struct PlaywrightSearchConfig<'a> {
    pub(super) query_variants: Option<&'a [String]>,
    pub(super) search_base_url: Option<&'a str>,
    pub(super) max_results: Option<usize>,
    pub(super) max_results_per_query: Option<usize>,
    pub(super) max_per_domain: Option<usize>,
    pub(super) max_content_chars: Option<usize>,
    pub(super) include_domains: Option<&'a [String]>,
    pub(super) exclude_domains: Option<&'a [String]>,
    pub(super) required_terms: Option<&'a [String]>,
    pub(super) exclude_terms: Option<&'a [String]>,
    pub(super) page_concurrency: Option<usize>,
    pub(super) navigation_timeout_ms: Option<u64>,
    pub(super) overall_timeout_ms: Option<u64>,
    pub(super) search_delay_ms: Option<u64>,
    pub(super) retry_count: Option<usize>,
    pub(super) user_agent: Option<&'a str>,
    pub(super) fetch_pages: Option<bool>,
    pub(super) cache_dir: Option<PathBuf>,
    pub(super) cache_ttl_seconds: Option<u64>,
    pub(super) timeout_seconds: Option<u64>,
    pub(super) action_timeout: Option<Duration>,
}

pub(super) struct PlaywrightPageAuditConfig<'a> {
    pub(super) script_path: &'a Path,
    pub(super) base_dir: &'a Path,
    pub(super) screenshot_dir: Option<PathBuf>,
    pub(super) viewports: Option<&'a [Value]>,
    pub(super) required_text: Option<&'a [String]>,
    pub(super) forbidden_text: Option<&'a [String]>,
    pub(super) require_canvas: Option<bool>,
    pub(super) navigation_timeout_ms: Option<u64>,
    pub(super) max_text_chars: Option<usize>,
    pub(super) timeout_seconds: Option<u64>,
    pub(super) action_timeout: Option<Duration>,
}

fn compute_request_timeout(
    timeout_seconds: Option<u64>,
    action_timeout: Option<Duration>,
    default_timeout_seconds: u64,
) -> Duration {
    let configured_timeout =
        Duration::from_secs(timeout_seconds.unwrap_or(default_timeout_seconds));
    action_timeout
        .map(|timeout| timeout.min(configured_timeout))
        .unwrap_or(configured_timeout)
}

pub(super) fn run_playwright_subprocess(
    name: &str,
    label: &str,
    script: &Path,
    request: &Map<String, Value>,
    request_timeout: Duration,
) -> Result<Value, RuntimeError> {
    let mut child = Command::new("node")
        .arg(script)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| {
            RuntimeError::Provider(format!("tool {name} launch playwright {label}: {error}"))
        })?;
    {
        let mut stdin = child.stdin.take().ok_or_else(|| {
            RuntimeError::Provider(format!("tool {name} playwright {label} stdin unavailable"))
        })?;
        serde_json::to_writer(&mut stdin, &Value::Object(request.clone())).map_err(|error| {
            RuntimeError::Provider(format!(
                "tool {name} write playwright {label} input: {error}"
            ))
        })?;
    }

    let started_at = std::time::Instant::now();
    loop {
        match child.try_wait().map_err(|error| {
            RuntimeError::Provider(format!("tool {name} playwright {label} wait: {error}"))
        })? {
            Some(status) => {
                let output = child.wait_with_output().map_err(|error| {
                    RuntimeError::Provider(format!(
                        "tool {name} playwright {label} collect output: {error}"
                    ))
                })?;
                if !status.success() {
                    return Err(RuntimeError::Provider(format!(
                        "tool {name} playwright {label} failed: {}",
                        provider_error_snippet(&String::from_utf8_lossy(&output.stderr))
                    )));
                }
                return serde_json::from_slice(&output.stdout).map_err(|error| {
                    RuntimeError::Provider(format!(
                        "tool {name} playwright {label} output was not valid JSON: {error}; body={}",
                        provider_error_snippet(&String::from_utf8_lossy(&output.stdout))
                    ))
                });
            }
            None if started_at.elapsed() >= request_timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RuntimeError::Provider(format!(
                    "tool {name} playwright {label} exceeded timeout_seconds={}",
                    request_timeout.as_secs()
                )));
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

pub(super) fn call_playwright_page_audit_tool(
    name: &str,
    input: &Value,
    config: PlaywrightPageAuditConfig<'_>,
) -> Result<Value, RuntimeError> {
    let script = canonicalize_tool_path(name, "script_path", config.script_path)?;
    let base = canonicalize_tool_path(name, "base_dir", config.base_dir)?;
    let request_timeout =
        compute_request_timeout(config.timeout_seconds, config.action_timeout, 300);

    let mut request = Map::new();
    match (
        input.get("path").and_then(Value::as_str),
        input.get("url").and_then(Value::as_str),
    ) {
        (Some(path), _) if !path.trim().is_empty() => {
            let candidate = if Path::new(path).is_absolute() {
                PathBuf::from(path)
            } else {
                base.join(path)
            };
            let path = canonicalize_tool_path(name, "input.path", &candidate)?;
            if !path.starts_with(&base) {
                return Err(RuntimeError::Provider(format!(
                    "tool {name} input.path must stay inside base_dir"
                )));
            }
            request.insert(
                "path".to_string(),
                Value::String(path.display().to_string()),
            );
        }
        (_, Some(url)) if !url.trim().is_empty() => {
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(RuntimeError::Provider(format!(
                    "tool {name} input.url must start with http:// or https://"
                )));
            }
            request.insert("url".to_string(), Value::String(url.to_string()));
        }
        _ => {
            return Err(RuntimeError::Provider(format!(
                "tool {name} requires input.path or input.url"
            )))
        }
    }
    if let Some(value) = input.get("viewports") {
        request.insert("viewports".to_string(), value.clone());
    } else if let Some(viewports) = config.viewports {
        request.insert("viewports".to_string(), Value::Array(viewports.to_vec()));
    }
    insert_input_or_config_array(&mut request, input, "required_text", config.required_text);
    insert_input_or_config_array(&mut request, input, "forbidden_text", config.forbidden_text);
    if let Some(value) = input.get("require_canvas") {
        request.insert("require_canvas".to_string(), value.clone());
    } else if let Some(require_canvas) = config.require_canvas {
        request.insert("require_canvas".to_string(), Value::Bool(require_canvas));
    }
    if let Some(value) = input.get("navigation_timeout_ms") {
        request.insert("navigation_timeout_ms".to_string(), value.clone());
    } else if let Some(navigation_timeout_ms) = config.navigation_timeout_ms {
        request.insert(
            "navigation_timeout_ms".to_string(),
            Value::Number(navigation_timeout_ms.into()),
        );
    }
    if let Some(value) = input.get("max_text_chars") {
        request.insert("max_text_chars".to_string(), value.clone());
    } else if let Some(max_text_chars) = config.max_text_chars {
        request.insert(
            "max_text_chars".to_string(),
            Value::Number(max_text_chars.into()),
        );
    }
    if let Some(value) = input.get("screenshot_dir") {
        request.insert("screenshot_dir".to_string(), value.clone());
    } else if let Some(screenshot_dir) = config.screenshot_dir {
        request.insert(
            "screenshot_dir".to_string(),
            Value::String(screenshot_dir.display().to_string()),
        );
    }

    run_playwright_subprocess(name, "page audit", &script, &request, request_timeout)
}

pub(super) fn insert_numeric_request_option(
    request: &mut Map<String, Value>,
    input: &Value,
    key: &str,
    config_value: Option<u64>,
    default: u64,
) {
    request.insert(
        key.to_string(),
        input
            .get(key)
            .cloned()
            .unwrap_or_else(|| Value::from(config_value.unwrap_or(default))),
    );
}

pub(super) fn call_playwright_search_tool(
    name: &str,
    input: &Value,
    script_path: &Path,
    config: PlaywrightSearchConfig<'_>,
) -> Result<Value, RuntimeError> {
    let query = required_input_string(name, input, "query")?;
    let script = canonicalize_tool_path(name, "script_path", script_path)?;
    let request_timeout =
        compute_request_timeout(config.timeout_seconds, config.action_timeout, 600);
    let mut request = Map::new();
    request.insert("query".to_string(), Value::String(query.to_string()));
    insert_numeric_request_option(
        &mut request,
        input,
        "max_results",
        config.max_results.map(|v| v as u64),
        5,
    );
    insert_numeric_request_option(
        &mut request,
        input,
        "max_results_per_query",
        config.max_results_per_query.map(|v| v as u64),
        10,
    );
    insert_numeric_request_option(
        &mut request,
        input,
        "max_per_domain",
        config.max_per_domain.map(|v| v as u64),
        2,
    );
    insert_numeric_request_option(
        &mut request,
        input,
        "max_content_chars",
        config.max_content_chars.map(|v| v as u64),
        12_000,
    );
    insert_numeric_request_option(
        &mut request,
        input,
        "page_concurrency",
        config.page_concurrency.map(|v| v as u64),
        3,
    );
    insert_numeric_request_option(
        &mut request,
        input,
        "navigation_timeout_ms",
        config.navigation_timeout_ms,
        20_000,
    );
    request.insert(
        "overall_timeout_ms".to_string(),
        input.get("overall_timeout_ms").cloned().unwrap_or_else(|| {
            let configured = config.overall_timeout_ms.unwrap_or_else(|| {
                request_timeout
                    .saturating_sub(Duration::from_millis(500))
                    .as_millis()
                    .try_into()
                    .unwrap_or(u64::MAX)
            });
            Value::Number(configured.into())
        }),
    );
    insert_numeric_request_option(
        &mut request,
        input,
        "retry_count",
        config.retry_count.map(|v| v as u64),
        1,
    );
    insert_numeric_request_option(
        &mut request,
        input,
        "search_delay_ms",
        config.search_delay_ms,
        0,
    );
    if let Some(value) = input.get("user_agent") {
        request.insert("user_agent".to_string(), value.clone());
    } else if let Some(user_agent) = config.user_agent {
        request.insert(
            "user_agent".to_string(),
            Value::String(user_agent.to_string()),
        );
    }
    if let Some(value) = input.get("search_base_url") {
        request.insert("search_base_url".to_string(), value.clone());
    } else if let Some(search_base_url) = config.search_base_url {
        request.insert(
            "search_base_url".to_string(),
            Value::String(search_base_url.to_string()),
        );
    }
    if let Some(value) = input.get("fetch_pages") {
        request.insert("fetch_pages".to_string(), value.clone());
    } else if let Some(fetch_pages) = config.fetch_pages {
        request.insert("fetch_pages".to_string(), Value::Bool(fetch_pages));
    }
    if let Some(value) = input.get("cache_dir") {
        request.insert("cache_dir".to_string(), value.clone());
    } else if let Some(cache_dir) = config.cache_dir {
        request.insert(
            "cache_dir".to_string(),
            Value::String(cache_dir.display().to_string()),
        );
    }
    if let Some(value) = input.get("cache_ttl_seconds") {
        request.insert("cache_ttl_seconds".to_string(), value.clone());
    } else if let Some(cache_ttl_seconds) = config.cache_ttl_seconds {
        request.insert(
            "cache_ttl_seconds".to_string(),
            Value::Number(cache_ttl_seconds.into()),
        );
    }
    insert_input_or_config_array(&mut request, input, "query_variants", config.query_variants);
    insert_input_or_config_array(
        &mut request,
        input,
        "include_domains",
        config.include_domains,
    );
    insert_input_or_config_array(
        &mut request,
        input,
        "exclude_domains",
        config.exclude_domains,
    );
    insert_input_or_config_array(&mut request, input, "required_terms", config.required_terms);
    insert_input_or_config_array(&mut request, input, "exclude_terms", config.exclude_terms);

    run_playwright_subprocess(name, "search", &script, &request, request_timeout)
}

pub(super) fn insert_input_or_config_array(
    request: &mut Map<String, Value>,
    input: &Value,
    key: &str,
    configured: Option<&[String]>,
) {
    if let Some(value) = input.get(key) {
        request.insert(key.to_string(), value.clone());
    } else if let Some(values) = configured {
        request.insert(
            key.to_string(),
            Value::Array(
                values
                    .iter()
                    .map(|value| Value::String(render_json_template(value, input)))
                    .collect(),
            ),
        );
    }
}
