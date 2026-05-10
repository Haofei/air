use air_runtime::{
    sanitize_trace_text, ApprovalDecision, RuntimeError, ToolProvider, TraceWriteOptions,
};
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct EchoTools;

impl ToolProvider for EchoTools {
    fn call_tool(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        Ok(json!({
            "tool": name,
            "input": input
        }))
    }
}

#[derive(Clone)]
pub enum ToolProviderChoice {
    Echo(EchoTools),
    Config(ConfigTools),
}

impl ToolProviderChoice {
    pub fn from_config(tool_config: Option<PathBuf>, example_tools: bool) -> Result<Self> {
        if let Some(tool_config) = tool_config {
            Ok(Self::Config(ConfigTools::from_file(tool_config)?))
        } else if example_tools {
            Ok(Self::Config(ConfigTools::example()))
        } else {
            Ok(Self::Echo(EchoTools))
        }
    }
}

impl ToolProvider for ToolProviderChoice {
    fn call_tool(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        match self {
            ToolProviderChoice::Echo(provider) => provider.call_tool(name, input),
            ToolProviderChoice::Config(provider) => provider.call_tool(name, input),
        }
    }

    fn call_tool_with_timeout(
        &mut self,
        name: &str,
        input: &Value,
        timeout: Duration,
    ) -> Result<Value, RuntimeError> {
        match self {
            ToolProviderChoice::Echo(provider) => {
                provider.call_tool_with_timeout(name, input, timeout)
            }
            ToolProviderChoice::Config(provider) => {
                provider.call_tool_with_timeout(name, input, timeout)
            }
        }
    }

    fn tool_capability(&self, name: &str) -> Option<&str> {
        match self {
            ToolProviderChoice::Echo(provider) => provider.tool_capability(name),
            ToolProviderChoice::Config(provider) => provider.tool_capability(name),
        }
    }

    fn request_approval(
        &mut self,
        module: &air_core::AirModule,
        approval_for: &[String],
        state: &Value,
    ) -> Result<ApprovalDecision, RuntimeError> {
        match self {
            ToolProviderChoice::Echo(provider) => {
                provider.request_approval(module, approval_for, state)
            }
            ToolProviderChoice::Config(provider) => {
                provider.request_approval(module, approval_for, state)
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct ToolConfigFile {
    #[serde(default)]
    tools: BTreeMap<String, ToolConfig>,

    #[serde(default)]
    approvals: BTreeMap<String, ApprovalConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
enum ToolConfig {
    LocalDocsSearch {
        #[serde(default)]
        capability: Option<String>,

        documents: Vec<LocalDoc>,

        #[serde(default)]
        max_results: Option<usize>,
    },
    LocalReflection {
        #[serde(default)]
        capability: Option<String>,
    },
    HttpJson {
        #[serde(default)]
        capability: Option<String>,

        url: String,

        #[serde(default = "default_http_method")]
        method: String,

        #[serde(default)]
        headers: BTreeMap<String, String>,

        #[serde(default)]
        bearer_token_env: Option<String>,

        #[serde(default)]
        body: Option<Value>,

        #[serde(default)]
        timeout_seconds: Option<u64>,
    },
    WebFetch {
        #[serde(default)]
        capability: Option<String>,

        #[serde(default)]
        headers: BTreeMap<String, String>,

        #[serde(default)]
        bearer_token_env: Option<String>,

        #[serde(default)]
        timeout_seconds: Option<u64>,

        #[serde(default)]
        max_bytes: Option<usize>,
    },
    PlaywrightSearch {
        #[serde(default)]
        capability: Option<String>,

        script_path: PathBuf,

        #[serde(default)]
        query_variants: Option<Vec<String>>,

        #[serde(default)]
        search_base_url: Option<String>,

        #[serde(default)]
        max_results: Option<usize>,

        #[serde(default)]
        max_results_per_query: Option<usize>,

        #[serde(default)]
        max_per_domain: Option<usize>,

        #[serde(default)]
        max_content_chars: Option<usize>,

        #[serde(default)]
        include_domains: Option<Vec<String>>,

        #[serde(default)]
        exclude_domains: Option<Vec<String>>,

        #[serde(default)]
        required_terms: Option<Vec<String>>,

        #[serde(default)]
        exclude_terms: Option<Vec<String>>,

        #[serde(default)]
        page_concurrency: Option<usize>,

        #[serde(default)]
        navigation_timeout_ms: Option<u64>,

        #[serde(default)]
        overall_timeout_ms: Option<u64>,

        #[serde(default)]
        search_delay_ms: Option<u64>,

        #[serde(default)]
        retry_count: Option<usize>,

        #[serde(default)]
        user_agent: Option<String>,

        #[serde(default)]
        fetch_pages: Option<bool>,

        #[serde(default)]
        cache_dir: Option<PathBuf>,

        #[serde(default)]
        cache_ttl_seconds: Option<u64>,

        #[serde(default)]
        timeout_seconds: Option<u64>,
    },
    PlaywrightPageAudit {
        #[serde(default)]
        capability: Option<String>,

        script_path: PathBuf,

        base_dir: PathBuf,

        #[serde(default)]
        screenshot_dir: Option<PathBuf>,

        #[serde(default)]
        viewports: Option<Vec<Value>>,

        #[serde(default)]
        navigation_timeout_ms: Option<u64>,

        #[serde(default)]
        max_text_chars: Option<usize>,

        #[serde(default)]
        timeout_seconds: Option<u64>,
    },
    FileRead {
        #[serde(default)]
        capability: Option<String>,

        base_dir: PathBuf,

        #[serde(default)]
        max_bytes: Option<usize>,
    },
    FileReadMany {
        #[serde(default)]
        capability: Option<String>,

        base_dir: PathBuf,

        #[serde(default)]
        max_bytes: Option<usize>,

        #[serde(default)]
        max_files: Option<usize>,
    },
    FileWrite {
        #[serde(default)]
        capability: Option<String>,

        base_dir: PathBuf,

        #[serde(default)]
        max_bytes: Option<usize>,

        #[serde(default)]
        create_dirs: Option<bool>,

        #[serde(default)]
        allow_overwrite: Option<bool>,

        #[serde(default)]
        require_read: Option<bool>,
    },
    FileEdit {
        #[serde(default)]
        capability: Option<String>,

        base_dir: PathBuf,

        #[serde(default)]
        max_bytes: Option<usize>,

        #[serde(default)]
        require_read: Option<bool>,

        #[serde(default)]
        allow_replace_all: Option<bool>,
    },
    FilePatch {
        #[serde(default)]
        capability: Option<String>,

        repo_dir: PathBuf,

        #[serde(default)]
        max_bytes: Option<usize>,

        #[serde(default)]
        max_files: Option<usize>,

        #[serde(default)]
        require_read: Option<bool>,

        #[serde(default)]
        allow_new_files: Option<bool>,

        #[serde(default)]
        allow_delete_files: Option<bool>,
    },
    GitDiff {
        #[serde(default)]
        capability: Option<String>,

        repo_dir: PathBuf,

        #[serde(default)]
        max_bytes: Option<usize>,
    },
    GitStatus {
        #[serde(default)]
        capability: Option<String>,

        repo_dir: PathBuf,

        #[serde(default)]
        max_files: Option<usize>,
    },
    RepoFiles {
        #[serde(default)]
        capability: Option<String>,

        repo_dir: PathBuf,

        #[serde(default)]
        max_files: Option<usize>,
    },
    RepoSearch {
        #[serde(default)]
        capability: Option<String>,

        repo_dir: PathBuf,

        #[serde(default)]
        max_matches: Option<usize>,

        #[serde(default)]
        max_bytes: Option<usize>,
    },
    RepoContext {
        #[serde(default)]
        capability: Option<String>,

        repo_dir: PathBuf,

        #[serde(default)]
        max_matches: Option<usize>,

        #[serde(default)]
        max_files: Option<usize>,

        #[serde(default)]
        context_lines: Option<usize>,

        #[serde(default)]
        max_bytes: Option<usize>,
    },
    RepoSymbols {
        #[serde(default)]
        capability: Option<String>,

        repo_dir: PathBuf,

        #[serde(default)]
        max_symbols: Option<usize>,

        #[serde(default)]
        max_bytes: Option<usize>,
    },
    RepoReferences {
        #[serde(default)]
        capability: Option<String>,

        repo_dir: PathBuf,

        #[serde(default)]
        max_matches: Option<usize>,

        #[serde(default)]
        max_files: Option<usize>,

        #[serde(default)]
        context_lines: Option<usize>,

        #[serde(default)]
        max_bytes: Option<usize>,
    },
    DiagnosticContext {
        #[serde(default)]
        capability: Option<String>,

        repo_dir: PathBuf,

        #[serde(default)]
        max_diagnostics: Option<usize>,

        #[serde(default)]
        context_lines: Option<usize>,

        #[serde(default)]
        max_bytes: Option<usize>,
    },
    TodoWrite {
        #[serde(default)]
        capability: Option<String>,

        #[serde(default)]
        max_items: Option<usize>,

        #[serde(default)]
        max_content_chars: Option<usize>,
    },
    TodoRead {
        #[serde(default)]
        capability: Option<String>,
    },
    ContextMeasure {
        #[serde(default)]
        capability: Option<String>,

        #[serde(default)]
        max_context_chars: Option<usize>,

        #[serde(default)]
        threshold_percent: Option<u64>,
    },
    CommandRun {
        #[serde(default)]
        capability: Option<String>,

        cwd: PathBuf,

        commands: BTreeMap<String, Vec<String>>,

        #[serde(default)]
        timeout_seconds: Option<u64>,

        #[serde(default)]
        max_bytes: Option<usize>,
    },
}

impl ToolConfig {
    fn capability(&self) -> Option<&str> {
        match self {
            ToolConfig::LocalDocsSearch { capability, .. }
            | ToolConfig::LocalReflection { capability }
            | ToolConfig::HttpJson { capability, .. }
            | ToolConfig::WebFetch { capability, .. }
            | ToolConfig::PlaywrightSearch { capability, .. }
            | ToolConfig::PlaywrightPageAudit { capability, .. }
            | ToolConfig::FileRead { capability, .. }
            | ToolConfig::FileReadMany { capability, .. }
            | ToolConfig::FileWrite { capability, .. }
            | ToolConfig::FileEdit { capability, .. }
            | ToolConfig::FilePatch { capability, .. }
            | ToolConfig::GitDiff { capability, .. }
            | ToolConfig::GitStatus { capability, .. }
            | ToolConfig::RepoFiles { capability, .. }
            | ToolConfig::RepoSearch { capability, .. }
            | ToolConfig::RepoContext { capability, .. }
            | ToolConfig::RepoSymbols { capability, .. }
            | ToolConfig::RepoReferences { capability, .. }
            | ToolConfig::DiagnosticContext { capability, .. }
            | ToolConfig::TodoWrite { capability, .. }
            | ToolConfig::TodoRead { capability }
            | ToolConfig::ContextMeasure { capability, .. }
            | ToolConfig::CommandRun { capability, .. } => capability.as_deref(),
        }
    }
}

fn default_http_method() -> String {
    "POST".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct LocalDoc {
    pub id: String,
    pub title: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ApprovalConfig {
    approved: bool,

    #[serde(default)]
    approver: Option<String>,

    #[serde(default)]
    reason: Option<String>,

    #[serde(default)]
    metadata: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct ConfigTools {
    tools: BTreeMap<String, ToolConfig>,
    approvals: BTreeMap<String, ApprovalConfig>,
    config_dir: PathBuf,
    read_snapshots: BTreeMap<PathBuf, SystemTime>,
    current_todos: Vec<Value>,
}

impl ConfigTools {
    pub fn from_file(path: PathBuf) -> Result<Self> {
        let source = fs::read_to_string(&path)
            .with_context(|| format!("failed to read tool config {}", path.display()))?;
        let config: ToolConfigFile = serde_json::from_str(&source)
            .with_context(|| format!("failed to parse tool config JSON {}", path.display()))?;
        validate_tool_config(&config, &path)?;
        Ok(Self {
            tools: config.tools,
            approvals: config.approvals,
            config_dir: path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf(),
            read_snapshots: BTreeMap::new(),
            current_todos: Vec::new(),
        })
    }

    fn example() -> Self {
        let docs = helpdesk_docs()
            .into_iter()
            .map(|doc| LocalDoc {
                id: doc.id.to_string(),
                title: doc.title.to_string(),
                content: doc.content.to_string(),
            })
            .collect();
        Self {
            tools: BTreeMap::from([(
                "docs.search".to_string(),
                ToolConfig::LocalDocsSearch {
                    capability: Some("retrieval.local".to_string()),
                    documents: docs,
                    max_results: None,
                },
            )]),
            approvals: BTreeMap::new(),
            config_dir: PathBuf::from("."),
            read_snapshots: BTreeMap::new(),
            current_todos: Vec::new(),
        }
    }

    fn remember_read_snapshot(&mut self, path: &Path) -> Result<(), RuntimeError> {
        let path = fs::canonicalize(path).map_err(|error| {
            RuntimeError::Provider(format!("tool file snapshot canonicalize path: {error}"))
        })?;
        let modified = file_modified_time("tool", "path", &path)?;
        self.read_snapshots.insert(path, modified);
        Ok(())
    }
}

fn validate_tool_config(config: &ToolConfigFile, path: &Path) -> Result<()> {
    for (name, tool) in &config.tools {
        if name.trim().is_empty() {
            anyhow::bail!(
                "tool config {} contains a tool with an empty name",
                path.display()
            );
        }
        validate_capability(path, &format!("tools.{name}.capability"), tool.capability())?;
        match tool {
            ToolConfig::LocalDocsSearch {
                documents,
                max_results,
                ..
            } => {
                if documents.is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.documents must contain at least one document",
                        path.display()
                    );
                }
                if max_results.is_some_and(|value| value == 0) {
                    anyhow::bail!(
                        "tool config {} tools.{name}.max_results must be greater than 0",
                        path.display()
                    );
                }
                for (index, document) in documents.iter().enumerate() {
                    if document.id.trim().is_empty() {
                        anyhow::bail!(
                            "tool config {} tools.{name}.documents[{index}].id must not be empty",
                            path.display()
                        );
                    }
                    if document.title.trim().is_empty() {
                        anyhow::bail!(
                            "tool config {} tools.{name}.documents[{index}].title must not be empty",
                            path.display()
                        );
                    }
                }
            }
            ToolConfig::LocalReflection { .. } => {}
            ToolConfig::HttpJson {
                url,
                method,
                headers,
                bearer_token_env,
                timeout_seconds,
                ..
            } => {
                if url.trim().is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.url must not be empty",
                        path.display()
                    );
                }
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    anyhow::bail!(
                        "tool config {} tools.{name}.url must start with http:// or https://",
                        path.display()
                    );
                }
                let method = method.to_ascii_uppercase();
                method.parse::<reqwest::Method>().with_context(|| {
                    format!(
                        "tool config {} tools.{name}.method is not a valid HTTP method",
                        path.display()
                    )
                })?;
                if headers.keys().any(|header| header.trim().is_empty()) {
                    anyhow::bail!(
                        "tool config {} tools.{name}.headers contains an empty header name",
                        path.display()
                    );
                }
                if bearer_token_env
                    .as_deref()
                    .is_some_and(|env_name| env_name.trim().is_empty())
                {
                    anyhow::bail!(
                        "tool config {} tools.{name}.bearer_token_env must not be empty",
                        path.display()
                    );
                }
                if timeout_seconds.is_some_and(|value| value == 0) {
                    anyhow::bail!(
                        "tool config {} tools.{name}.timeout_seconds must be greater than 0",
                        path.display()
                    );
                }
            }
            ToolConfig::WebFetch {
                headers,
                bearer_token_env,
                timeout_seconds,
                max_bytes,
                ..
            } => {
                if headers.keys().any(|header| header.trim().is_empty()) {
                    anyhow::bail!(
                        "tool config {} tools.{name}.headers contains an empty header name",
                        path.display()
                    );
                }
                if bearer_token_env
                    .as_deref()
                    .is_some_and(|env_name| env_name.trim().is_empty())
                {
                    anyhow::bail!(
                        "tool config {} tools.{name}.bearer_token_env must not be empty",
                        path.display()
                    );
                }
                validate_positive_u64(
                    path,
                    &format!("tools.{name}.timeout_seconds"),
                    *timeout_seconds,
                )?;
                validate_positive_usize(path, &format!("tools.{name}.max_bytes"), *max_bytes)?;
            }
            ToolConfig::PlaywrightSearch {
                script_path,
                query_variants,
                search_base_url,
                max_results,
                max_results_per_query,
                max_per_domain,
                max_content_chars,
                include_domains,
                exclude_domains,
                required_terms,
                exclude_terms,
                page_concurrency,
                navigation_timeout_ms,
                overall_timeout_ms,
                search_delay_ms,
                retry_count,
                user_agent,
                fetch_pages: _,
                cache_dir,
                cache_ttl_seconds,
                timeout_seconds,
                ..
            } => {
                if script_path.as_os_str().is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.script_path must not be empty",
                        path.display()
                    );
                }
                validate_non_empty_strings(
                    path,
                    &format!("tools.{name}.query_variants"),
                    query_variants.as_deref(),
                )?;
                if let Some(search_base_url) = search_base_url {
                    let value = search_base_url.trim();
                    if value.is_empty()
                        || !(value.starts_with("http://") || value.starts_with("https://"))
                    {
                        anyhow::bail!(
                            "tool config {} tools.{name}.search_base_url must be an http(s) URL",
                            path.display()
                        );
                    }
                }
                validate_positive_usize(path, &format!("tools.{name}.max_results"), *max_results)?;
                validate_positive_usize(
                    path,
                    &format!("tools.{name}.max_results_per_query"),
                    *max_results_per_query,
                )?;
                validate_positive_usize(
                    path,
                    &format!("tools.{name}.max_per_domain"),
                    *max_per_domain,
                )?;
                validate_positive_usize(
                    path,
                    &format!("tools.{name}.max_content_chars"),
                    *max_content_chars,
                )?;
                validate_domain_filters(
                    path,
                    &format!("tools.{name}.include_domains"),
                    include_domains.as_deref(),
                )?;
                validate_domain_filters(
                    path,
                    &format!("tools.{name}.exclude_domains"),
                    exclude_domains.as_deref(),
                )?;
                validate_non_empty_strings(
                    path,
                    &format!("tools.{name}.required_terms"),
                    required_terms.as_deref(),
                )?;
                validate_non_empty_strings(
                    path,
                    &format!("tools.{name}.exclude_terms"),
                    exclude_terms.as_deref(),
                )?;
                validate_positive_usize(
                    path,
                    &format!("tools.{name}.page_concurrency"),
                    *page_concurrency,
                )?;
                validate_positive_u64(
                    path,
                    &format!("tools.{name}.navigation_timeout_ms"),
                    *navigation_timeout_ms,
                )?;
                validate_positive_u64(
                    path,
                    &format!("tools.{name}.overall_timeout_ms"),
                    *overall_timeout_ms,
                )?;
                validate_non_negative_u64(
                    path,
                    &format!("tools.{name}.search_delay_ms"),
                    *search_delay_ms,
                )?;
                validate_retry_count(path, &format!("tools.{name}.retry_count"), *retry_count)?;
                if user_agent
                    .as_deref()
                    .is_some_and(|value| value.trim().is_empty())
                {
                    anyhow::bail!(
                        "tool config {} tools.{name}.user_agent must not be empty when provided",
                        path.display()
                    );
                }
                if cache_dir
                    .as_ref()
                    .is_some_and(|value| value.as_os_str().is_empty())
                {
                    anyhow::bail!(
                        "tool config {} tools.{name}.cache_dir must not be empty when provided",
                        path.display()
                    );
                }
                validate_positive_u64(
                    path,
                    &format!("tools.{name}.cache_ttl_seconds"),
                    *cache_ttl_seconds,
                )?;
                validate_positive_u64(
                    path,
                    &format!("tools.{name}.timeout_seconds"),
                    *timeout_seconds,
                )?;
            }
            ToolConfig::PlaywrightPageAudit {
                script_path,
                base_dir,
                screenshot_dir,
                viewports,
                navigation_timeout_ms,
                max_text_chars,
                timeout_seconds,
                ..
            } => {
                if script_path.as_os_str().is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.script_path must not be empty",
                        path.display()
                    );
                }
                if base_dir.as_os_str().is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.base_dir must not be empty",
                        path.display()
                    );
                }
                if screenshot_dir
                    .as_ref()
                    .is_some_and(|value| value.as_os_str().is_empty())
                {
                    anyhow::bail!(
                        "tool config {} tools.{name}.screenshot_dir must not be empty when provided",
                        path.display()
                    );
                }
                if let Some(viewports) = viewports {
                    if viewports.is_empty() {
                        anyhow::bail!(
                            "tool config {} tools.{name}.viewports must not be empty when provided",
                            path.display()
                        );
                    }
                }
                validate_positive_u64(
                    path,
                    &format!("tools.{name}.navigation_timeout_ms"),
                    *navigation_timeout_ms,
                )?;
                validate_positive_usize(
                    path,
                    &format!("tools.{name}.max_text_chars"),
                    *max_text_chars,
                )?;
                validate_positive_u64(
                    path,
                    &format!("tools.{name}.timeout_seconds"),
                    *timeout_seconds,
                )?;
            }
            ToolConfig::FileRead {
                base_dir,
                max_bytes,
                ..
            } => {
                if base_dir.as_os_str().is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.base_dir must not be empty",
                        path.display()
                    );
                }
                validate_positive_usize(path, &format!("tools.{name}.max_bytes"), *max_bytes)?;
            }
            ToolConfig::FileReadMany {
                base_dir,
                max_bytes,
                max_files,
                ..
            } => {
                if base_dir.as_os_str().is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.base_dir must not be empty",
                        path.display()
                    );
                }
                validate_positive_usize(path, &format!("tools.{name}.max_bytes"), *max_bytes)?;
                validate_positive_usize(path, &format!("tools.{name}.max_files"), *max_files)?;
            }
            ToolConfig::FileWrite {
                base_dir,
                max_bytes,
                ..
            } => {
                if base_dir.as_os_str().is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.base_dir must not be empty",
                        path.display()
                    );
                }
                validate_positive_usize(path, &format!("tools.{name}.max_bytes"), *max_bytes)?;
            }
            ToolConfig::FileEdit {
                base_dir,
                max_bytes,
                ..
            } => {
                if base_dir.as_os_str().is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.base_dir must not be empty",
                        path.display()
                    );
                }
                validate_positive_usize(path, &format!("tools.{name}.max_bytes"), *max_bytes)?;
            }
            ToolConfig::FilePatch {
                repo_dir,
                max_bytes,
                max_files,
                ..
            } => {
                if repo_dir.as_os_str().is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.repo_dir must not be empty",
                        path.display()
                    );
                }
                validate_positive_usize(path, &format!("tools.{name}.max_bytes"), *max_bytes)?;
                validate_positive_usize(path, &format!("tools.{name}.max_files"), *max_files)?;
            }
            ToolConfig::GitDiff {
                repo_dir,
                max_bytes,
                ..
            } => {
                if repo_dir.as_os_str().is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.repo_dir must not be empty",
                        path.display()
                    );
                }
                validate_positive_usize(path, &format!("tools.{name}.max_bytes"), *max_bytes)?;
            }
            ToolConfig::GitStatus {
                repo_dir,
                max_files,
                ..
            } => {
                if repo_dir.as_os_str().is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.repo_dir must not be empty",
                        path.display()
                    );
                }
                validate_positive_usize(path, &format!("tools.{name}.max_files"), *max_files)?;
            }
            ToolConfig::RepoFiles {
                repo_dir,
                max_files,
                ..
            } => {
                if repo_dir.as_os_str().is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.repo_dir must not be empty",
                        path.display()
                    );
                }
                validate_positive_usize(path, &format!("tools.{name}.max_files"), *max_files)?;
            }
            ToolConfig::RepoSearch {
                repo_dir,
                max_matches,
                max_bytes,
                ..
            } => {
                if repo_dir.as_os_str().is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.repo_dir must not be empty",
                        path.display()
                    );
                }
                validate_positive_usize(path, &format!("tools.{name}.max_matches"), *max_matches)?;
                validate_positive_usize(path, &format!("tools.{name}.max_bytes"), *max_bytes)?;
            }
            ToolConfig::RepoContext {
                repo_dir,
                max_matches,
                max_files,
                context_lines,
                max_bytes,
                ..
            } => {
                if repo_dir.as_os_str().is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.repo_dir must not be empty",
                        path.display()
                    );
                }
                validate_positive_usize(path, &format!("tools.{name}.max_matches"), *max_matches)?;
                validate_positive_usize(path, &format!("tools.{name}.max_files"), *max_files)?;
                validate_positive_usize(
                    path,
                    &format!("tools.{name}.context_lines"),
                    *context_lines,
                )?;
                validate_positive_usize(path, &format!("tools.{name}.max_bytes"), *max_bytes)?;
            }
            ToolConfig::RepoSymbols {
                repo_dir,
                max_symbols,
                max_bytes,
                ..
            } => {
                if repo_dir.as_os_str().is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.repo_dir must not be empty",
                        path.display()
                    );
                }
                validate_positive_usize(path, &format!("tools.{name}.max_symbols"), *max_symbols)?;
                validate_positive_usize(path, &format!("tools.{name}.max_bytes"), *max_bytes)?;
            }
            ToolConfig::RepoReferences {
                repo_dir,
                max_matches,
                max_files,
                context_lines,
                max_bytes,
                ..
            } => {
                if repo_dir.as_os_str().is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.repo_dir must not be empty",
                        path.display()
                    );
                }
                validate_positive_usize(path, &format!("tools.{name}.max_matches"), *max_matches)?;
                validate_positive_usize(path, &format!("tools.{name}.max_files"), *max_files)?;
                validate_positive_usize(
                    path,
                    &format!("tools.{name}.context_lines"),
                    *context_lines,
                )?;
                validate_positive_usize(path, &format!("tools.{name}.max_bytes"), *max_bytes)?;
            }
            ToolConfig::DiagnosticContext {
                repo_dir,
                max_diagnostics,
                context_lines,
                max_bytes,
                ..
            } => {
                if repo_dir.as_os_str().is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.repo_dir must not be empty",
                        path.display()
                    );
                }
                validate_positive_usize(
                    path,
                    &format!("tools.{name}.max_diagnostics"),
                    *max_diagnostics,
                )?;
                validate_positive_usize(
                    path,
                    &format!("tools.{name}.context_lines"),
                    *context_lines,
                )?;
                validate_positive_usize(path, &format!("tools.{name}.max_bytes"), *max_bytes)?;
            }
            ToolConfig::TodoWrite {
                max_items,
                max_content_chars,
                ..
            } => {
                validate_positive_usize(path, &format!("tools.{name}.max_items"), *max_items)?;
                validate_positive_usize(
                    path,
                    &format!("tools.{name}.max_content_chars"),
                    *max_content_chars,
                )?;
            }
            ToolConfig::TodoRead { .. } => {}
            ToolConfig::ContextMeasure {
                max_context_chars,
                threshold_percent,
                ..
            } => {
                validate_positive_usize(
                    path,
                    &format!("tools.{name}.max_context_chars"),
                    *max_context_chars,
                )?;
                validate_threshold_percent(
                    path,
                    &format!("tools.{name}.threshold_percent"),
                    *threshold_percent,
                )?;
            }
            ToolConfig::CommandRun {
                cwd,
                commands,
                timeout_seconds,
                max_bytes,
                ..
            } => {
                if cwd.as_os_str().is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.cwd must not be empty",
                        path.display()
                    );
                }
                if commands.is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.commands must not be empty",
                        path.display()
                    );
                }
                for (alias, command) in commands {
                    if alias.trim().is_empty() {
                        anyhow::bail!(
                            "tool config {} tools.{name}.commands contains an empty alias",
                            path.display()
                        );
                    }
                    if command.is_empty() || command.iter().any(|part| part.trim().is_empty()) {
                        anyhow::bail!(
                            "tool config {} tools.{name}.commands.{alias} must contain non-empty command parts",
                            path.display()
                        );
                    }
                }
                validate_positive_u64(
                    path,
                    &format!("tools.{name}.timeout_seconds"),
                    *timeout_seconds,
                )?;
                validate_positive_usize(path, &format!("tools.{name}.max_bytes"), *max_bytes)?;
            }
        }
    }

    for capability in config.approvals.keys() {
        if capability.trim().is_empty() {
            anyhow::bail!(
                "tool config {} approvals contains an empty capability name",
                path.display()
            );
        }
    }

    Ok(())
}

fn validate_positive_u64(path: &Path, field: &str, value: Option<u64>) -> Result<()> {
    if value.is_some_and(|value| value == 0) {
        anyhow::bail!(
            "tool config {} {field} must be greater than 0",
            path.display()
        );
    }
    Ok(())
}

fn validate_non_negative_u64(path: &Path, field: &str, value: Option<u64>) -> Result<()> {
    if value.is_some_and(|value| value > 300_000) {
        anyhow::bail!(
            "tool config {} {field} must be less than or equal to 300000",
            path.display()
        );
    }
    Ok(())
}

fn validate_positive_usize(path: &Path, field: &str, value: Option<usize>) -> Result<()> {
    if value.is_some_and(|value| value == 0) {
        anyhow::bail!(
            "tool config {} {field} must be greater than 0",
            path.display()
        );
    }
    Ok(())
}

fn validate_threshold_percent(path: &Path, field: &str, value: Option<u64>) -> Result<()> {
    if value.is_some_and(|value| value == 0 || value > 100) {
        anyhow::bail!(
            "tool config {} {field} must be between 1 and 100",
            path.display()
        );
    }
    Ok(())
}

fn validate_retry_count(path: &Path, field: &str, value: Option<usize>) -> Result<()> {
    if value.is_some_and(|value| value > 32) {
        anyhow::bail!(
            "tool config {} {field} must be less than or equal to 32",
            path.display()
        );
    }
    Ok(())
}

fn validate_non_empty_strings(path: &Path, field: &str, values: Option<&[String]>) -> Result<()> {
    if values
        .unwrap_or_default()
        .iter()
        .any(|value| value.trim().is_empty())
    {
        anyhow::bail!(
            "tool config {} {field} entries must not be empty",
            path.display()
        );
    }
    Ok(())
}

fn validate_domain_filters(path: &Path, field: &str, values: Option<&[String]>) -> Result<()> {
    let Some(values) = values else {
        return Ok(());
    };
    for value in values {
        let domain = value.trim();
        if domain.is_empty()
            || domain.contains('/')
            || domain.contains(':')
            || domain.contains(char::is_whitespace)
        {
            anyhow::bail!(
                "tool config {} {field} entries must be hostnames or hostname suffixes",
                path.display()
            );
        }
    }
    Ok(())
}

fn validate_capability(path: &Path, field: &str, capability: Option<&str>) -> Result<()> {
    if capability.is_some_and(|value| value.trim().is_empty()) {
        anyhow::bail!(
            "tool config {} {field} must not be empty when provided",
            path.display()
        );
    }
    Ok(())
}

impl ToolProvider for ConfigTools {
    fn call_tool(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        self.call_tool_with_timeout(name, input, Duration::from_secs(30))
    }

    fn call_tool_with_timeout(
        &mut self,
        name: &str,
        input: &Value,
        timeout: Duration,
    ) -> Result<Value, RuntimeError> {
        let Some(tool) = self.tools.get(name).cloned() else {
            return Err(RuntimeError::Provider(format!(
                "unknown configured tool {name}"
            )));
        };
        match tool {
            ToolConfig::LocalDocsSearch {
                capability: _,
                documents,
                max_results,
            } => Ok(search_docs(input, &documents, max_results.unwrap_or(3))),
            ToolConfig::LocalReflection { capability: _ } => Ok(json!({
                "reflection": input
                    .get("reflection")
                    .cloned()
                    .unwrap_or(Value::String(String::new()))
            })),
            ToolConfig::HttpJson {
                capability: _,
                url,
                method,
                headers,
                bearer_token_env,
                body,
                timeout_seconds,
            } => call_http_json_tool(
                name,
                input,
                HttpJsonToolConfig {
                    url: &url,
                    method: &method,
                    headers: &headers,
                    bearer_token_env: bearer_token_env.as_deref(),
                    body: body.as_ref(),
                    timeout_seconds,
                    action_timeout: Some(timeout),
                },
            ),
            ToolConfig::WebFetch {
                capability: _,
                headers,
                bearer_token_env,
                timeout_seconds,
                max_bytes,
            } => call_web_fetch_tool(
                name,
                input,
                WebFetchToolConfig {
                    headers: &headers,
                    bearer_token_env: bearer_token_env.as_deref(),
                    timeout_seconds,
                    action_timeout: Some(timeout),
                    max_bytes: max_bytes.unwrap_or(256 * 1024),
                },
            ),
            ToolConfig::PlaywrightSearch {
                capability: _,
                script_path,
                query_variants,
                search_base_url,
                max_results,
                max_results_per_query,
                max_per_domain,
                max_content_chars,
                include_domains,
                exclude_domains,
                required_terms,
                exclude_terms,
                page_concurrency,
                navigation_timeout_ms,
                overall_timeout_ms,
                search_delay_ms,
                retry_count,
                user_agent,
                fetch_pages,
                cache_dir,
                cache_ttl_seconds,
                timeout_seconds,
            } => call_playwright_search_tool(
                name,
                input,
                &resolve_config_path(&self.config_dir, &script_path),
                PlaywrightSearchConfig {
                    query_variants: query_variants.as_deref(),
                    search_base_url: search_base_url.as_deref(),
                    max_results,
                    max_results_per_query,
                    max_per_domain,
                    max_content_chars,
                    include_domains: include_domains.as_deref(),
                    exclude_domains: exclude_domains.as_deref(),
                    required_terms: required_terms.as_deref(),
                    exclude_terms: exclude_terms.as_deref(),
                    page_concurrency,
                    navigation_timeout_ms,
                    overall_timeout_ms,
                    search_delay_ms,
                    retry_count,
                    user_agent: user_agent.as_deref(),
                    fetch_pages,
                    cache_dir: cache_dir
                        .as_deref()
                        .map(|path| resolve_config_path(&self.config_dir, path)),
                    cache_ttl_seconds,
                    timeout_seconds,
                    action_timeout: Some(timeout),
                },
            ),
            ToolConfig::PlaywrightPageAudit {
                capability: _,
                script_path,
                base_dir,
                screenshot_dir,
                viewports,
                navigation_timeout_ms,
                max_text_chars,
                timeout_seconds,
            } => call_playwright_page_audit_tool(
                name,
                input,
                PlaywrightPageAuditConfig {
                    script_path: &resolve_config_path(&self.config_dir, &script_path),
                    base_dir: &resolve_config_path(&self.config_dir, &base_dir),
                    screenshot_dir: screenshot_dir
                        .as_deref()
                        .map(|path| resolve_config_path(&self.config_dir, path)),
                    viewports: viewports.as_deref(),
                    navigation_timeout_ms,
                    max_text_chars,
                    timeout_seconds,
                    action_timeout: Some(timeout),
                },
            ),
            ToolConfig::FileRead {
                capability: _,
                base_dir,
                max_bytes,
            } => {
                let base_dir = resolve_config_path(&self.config_dir, &base_dir);
                let output =
                    call_file_read_tool(name, input, &base_dir, max_bytes.unwrap_or(256 * 1024))?;
                if let Some(path) = output.get("path").and_then(Value::as_str) {
                    self.remember_read_snapshot(Path::new(path))?;
                }
                Ok(output)
            }
            ToolConfig::FileReadMany {
                capability: _,
                base_dir,
                max_bytes,
                max_files,
            } => {
                let base_dir = resolve_config_path(&self.config_dir, &base_dir);
                let output = call_file_read_many_tool(
                    name,
                    input,
                    &base_dir,
                    max_bytes.unwrap_or(64 * 1024),
                    max_files.unwrap_or(8),
                )?;
                for path in output
                    .get("files")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|file| file.get("path").and_then(Value::as_str))
                {
                    self.remember_read_snapshot(Path::new(path))?;
                }
                Ok(output)
            }
            ToolConfig::FileWrite {
                capability: _,
                base_dir,
                max_bytes,
                create_dirs,
                allow_overwrite,
                require_read,
            } => {
                let base_dir = resolve_config_path(&self.config_dir, &base_dir);
                let output = call_file_write_tool(
                    name,
                    input,
                    FileWriteOptions {
                        base_dir: &base_dir,
                        max_bytes: max_bytes.unwrap_or(256 * 1024),
                        create_dirs: create_dirs.unwrap_or(false),
                        allow_overwrite: allow_overwrite.unwrap_or(false),
                        require_read: require_read.unwrap_or(false),
                        read_snapshots: &self.read_snapshots,
                    },
                )?;
                if let Some(path) = output.get("path").and_then(Value::as_str) {
                    self.remember_read_snapshot(Path::new(path))?;
                }
                Ok(output)
            }
            ToolConfig::FileEdit {
                capability: _,
                base_dir,
                max_bytes,
                require_read,
                allow_replace_all,
            } => {
                let output = call_file_edit_tool(
                    name,
                    input,
                    &resolve_config_path(&self.config_dir, &base_dir),
                    max_bytes.unwrap_or(256 * 1024),
                    require_read.unwrap_or(true),
                    allow_replace_all.unwrap_or(false),
                    &self.read_snapshots,
                )?;
                if let Some(path) = output.get("path").and_then(Value::as_str) {
                    self.remember_read_snapshot(Path::new(path))?;
                }
                Ok(output)
            }
            ToolConfig::FilePatch {
                capability: _,
                repo_dir,
                max_bytes,
                max_files,
                require_read,
                allow_new_files,
                allow_delete_files,
            } => {
                let repo_dir = resolve_config_path(&self.config_dir, &repo_dir);
                let output = call_file_patch_tool(
                    name,
                    input,
                    FilePatchOptions {
                        repo_dir: &repo_dir,
                        max_bytes: max_bytes.unwrap_or(256 * 1024),
                        max_files: max_files.unwrap_or(20),
                        require_read: require_read.unwrap_or(true),
                        allow_new_files: allow_new_files.unwrap_or(true),
                        allow_delete_files: allow_delete_files.unwrap_or(false),
                        read_snapshots: &self.read_snapshots,
                    },
                )?;
                if output
                    .get("applied")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    if let Some(files) = output.get("files").and_then(Value::as_array) {
                        for file in files {
                            if file.get("kind").and_then(Value::as_str) == Some("delete") {
                                continue;
                            }
                            if let Some(path) = file.get("path").and_then(Value::as_str) {
                                self.remember_read_snapshot(&repo_dir.join(path))?;
                            }
                        }
                    }
                }
                Ok(output)
            }
            ToolConfig::GitDiff {
                capability: _,
                repo_dir,
                max_bytes,
            } => call_git_diff_tool(
                name,
                input,
                &resolve_config_path(&self.config_dir, &repo_dir),
                max_bytes.unwrap_or(256 * 1024),
            ),
            ToolConfig::GitStatus {
                capability: _,
                repo_dir,
                max_files,
            } => call_git_status_tool(
                name,
                &resolve_config_path(&self.config_dir, &repo_dir),
                max_files.unwrap_or(200),
            ),
            ToolConfig::RepoFiles {
                capability: _,
                repo_dir,
                max_files,
            } => call_repo_files_tool(
                name,
                input,
                &resolve_config_path(&self.config_dir, &repo_dir),
                max_files.unwrap_or(200),
            ),
            ToolConfig::RepoSearch {
                capability: _,
                repo_dir,
                max_matches,
                max_bytes,
            } => call_repo_search_tool(
                name,
                input,
                &resolve_config_path(&self.config_dir, &repo_dir),
                max_matches.unwrap_or(80),
                max_bytes.unwrap_or(256 * 1024),
            ),
            ToolConfig::RepoContext {
                capability: _,
                repo_dir,
                max_matches,
                max_files,
                context_lines,
                max_bytes,
            } => call_repo_context_tool(
                name,
                input,
                &resolve_config_path(&self.config_dir, &repo_dir),
                max_matches.unwrap_or(40),
                max_files.unwrap_or(8),
                context_lines.unwrap_or(6),
                max_bytes.unwrap_or(256 * 1024),
            ),
            ToolConfig::RepoSymbols {
                capability: _,
                repo_dir,
                max_symbols,
                max_bytes,
            } => call_repo_symbols_tool(
                name,
                input,
                &resolve_config_path(&self.config_dir, &repo_dir),
                max_symbols.unwrap_or(200),
                max_bytes.unwrap_or(256 * 1024),
            ),
            ToolConfig::RepoReferences {
                capability: _,
                repo_dir,
                max_matches,
                max_files,
                context_lines,
                max_bytes,
            } => call_repo_references_tool(
                name,
                input,
                &resolve_config_path(&self.config_dir, &repo_dir),
                max_matches.unwrap_or(120),
                max_files.unwrap_or(12),
                context_lines.unwrap_or(4),
                max_bytes.unwrap_or(256 * 1024),
            ),
            ToolConfig::DiagnosticContext {
                capability: _,
                repo_dir,
                max_diagnostics,
                context_lines,
                max_bytes,
            } => call_diagnostic_context_tool(
                name,
                input,
                &resolve_config_path(&self.config_dir, &repo_dir),
                max_diagnostics.unwrap_or(20),
                context_lines.unwrap_or(4),
                max_bytes.unwrap_or(256 * 1024),
            ),
            ToolConfig::TodoWrite {
                capability: _,
                max_items,
                max_content_chars,
            } => {
                let output = call_todo_write_tool(
                    name,
                    input,
                    max_items.unwrap_or(20),
                    max_content_chars.unwrap_or(200),
                )?;
                self.current_todos = output
                    .get("todos")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                Ok(output)
            }
            ToolConfig::TodoRead { capability: _ } => {
                Ok(call_todo_read_tool(name, &self.current_todos))
            }
            ToolConfig::ContextMeasure {
                capability: _,
                max_context_chars,
                threshold_percent,
            } => call_context_measure_tool(
                name,
                input,
                max_context_chars.unwrap_or(64 * 1024),
                threshold_percent.unwrap_or(80),
            ),
            ToolConfig::CommandRun {
                capability: _,
                cwd,
                commands,
                timeout_seconds,
                max_bytes,
            } => call_command_run_tool(
                name,
                input,
                &resolve_config_path(&self.config_dir, &cwd),
                &commands,
                timeout_seconds.unwrap_or(120),
                max_bytes.unwrap_or(256 * 1024),
            ),
        }
    }

    fn tool_capability(&self, name: &str) -> Option<&str> {
        match self.tools.get(name)? {
            ToolConfig::LocalDocsSearch { capability, .. }
            | ToolConfig::LocalReflection { capability }
            | ToolConfig::HttpJson { capability, .. }
            | ToolConfig::WebFetch { capability, .. }
            | ToolConfig::PlaywrightSearch { capability, .. }
            | ToolConfig::PlaywrightPageAudit { capability, .. }
            | ToolConfig::FileRead { capability, .. }
            | ToolConfig::FileReadMany { capability, .. }
            | ToolConfig::FileWrite { capability, .. }
            | ToolConfig::FileEdit { capability, .. }
            | ToolConfig::FilePatch { capability, .. }
            | ToolConfig::GitDiff { capability, .. }
            | ToolConfig::GitStatus { capability, .. }
            | ToolConfig::RepoFiles { capability, .. }
            | ToolConfig::RepoSearch { capability, .. }
            | ToolConfig::RepoContext { capability, .. }
            | ToolConfig::RepoSymbols { capability, .. }
            | ToolConfig::RepoReferences { capability, .. }
            | ToolConfig::DiagnosticContext { capability, .. }
            | ToolConfig::TodoWrite { capability, .. }
            | ToolConfig::TodoRead { capability }
            | ToolConfig::ContextMeasure { capability, .. }
            | ToolConfig::CommandRun { capability, .. } => capability.as_deref(),
        }
    }

    fn request_approval(
        &mut self,
        module: &air_core::AirModule,
        approval_for: &[String],
        _state: &Value,
    ) -> Result<ApprovalDecision, RuntimeError> {
        let mut decisions = Vec::new();
        let mut approver = None;
        let mut reason = None;
        for capability in approval_for {
            let Some(decision) = self.approvals.get(capability) else {
                return Err(RuntimeError::ApprovalRequired {
                    module: module.agent.name.clone(),
                    capabilities: approval_for.to_vec(),
                });
            };
            if !decision.approved {
                return Err(RuntimeError::ApprovalDenied {
                    module: module.agent.name.clone(),
                    capabilities: approval_for.to_vec(),
                    reason: decision
                        .reason
                        .clone()
                        .unwrap_or_else(|| "approval provider denied the request".to_string()),
                });
            }
            approver.get_or_insert_with(|| decision.approver.clone().unwrap_or_default());
            reason.get_or_insert_with(|| decision.reason.clone().unwrap_or_default());
            decisions.push(json!({
                "capability": capability,
                "approved": decision.approved,
                "approver": decision.approver,
                "reason": decision.reason,
                "metadata": decision.metadata,
            }));
        }

        Ok(ApprovalDecision {
            approved: true,
            approver: approver.filter(|value| !value.is_empty()),
            reason: reason.filter(|value| !value.is_empty()),
            metadata: Some(json!({ "decisions": decisions })),
        })
    }
}

struct HttpJsonToolConfig<'a> {
    url: &'a str,
    method: &'a str,
    headers: &'a BTreeMap<String, String>,
    bearer_token_env: Option<&'a str>,
    body: Option<&'a Value>,
    timeout_seconds: Option<u64>,
    action_timeout: Option<Duration>,
}

fn call_http_json_tool(
    name: &str,
    input: &Value,
    config: HttpJsonToolConfig<'_>,
) -> Result<Value, RuntimeError> {
    let method = config.method.to_ascii_uppercase();
    let configured_timeout = Duration::from_secs(config.timeout_seconds.unwrap_or(30));
    let request_timeout = config
        .action_timeout
        .map(|timeout| timeout.min(configured_timeout))
        .unwrap_or(configured_timeout);
    let client = reqwest::blocking::Client::builder()
        .timeout(request_timeout)
        .build()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} HTTP client: {error}")))?;
    let url = render_json_template(config.url, input);
    let request_method = method.parse::<reqwest::Method>().map_err(|error| {
        RuntimeError::Provider(format!("tool {name} invalid HTTP method {method}: {error}"))
    })?;
    let mut request = client.request(request_method, &url);
    for (header, value) in config.headers {
        request = request.header(header, render_json_template(value, input));
    }
    if let Some(env_name) = config.bearer_token_env {
        let token = std::env::var(env_name).map_err(|_| {
            RuntimeError::Provider(format!(
                "tool {name} requires bearer_token_env {env_name}, but it is not set"
            ))
        })?;
        request = request.bearer_auth(token);
    }
    if !matches!(method.as_str(), "GET" | "HEAD") {
        let body = config
            .body
            .map(|template| render_json_template_value(template, input))
            .unwrap_or_else(|| input.clone());
        request = request.json(&body);
    }

    let response = request
        .send()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} HTTP request: {error}")))?;
    let status = response.status();
    let body = response
        .text()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} HTTP response: {error}")))?;
    if !status.is_success() {
        return Err(RuntimeError::Provider(format!(
            "tool {name} HTTP status {}: {}",
            status.as_u16(),
            provider_error_snippet(&body)
        )));
    }
    serde_json::from_str(&body).map_err(|error| {
        RuntimeError::Provider(format!(
            "tool {name} HTTP response was not valid JSON: {error}; body={}",
            provider_error_snippet(&body)
        ))
    })
}

struct WebFetchToolConfig<'a> {
    headers: &'a BTreeMap<String, String>,
    bearer_token_env: Option<&'a str>,
    timeout_seconds: Option<u64>,
    action_timeout: Option<Duration>,
    max_bytes: usize,
}

struct PlaywrightSearchConfig<'a> {
    query_variants: Option<&'a [String]>,
    search_base_url: Option<&'a str>,
    max_results: Option<usize>,
    max_results_per_query: Option<usize>,
    max_per_domain: Option<usize>,
    max_content_chars: Option<usize>,
    include_domains: Option<&'a [String]>,
    exclude_domains: Option<&'a [String]>,
    required_terms: Option<&'a [String]>,
    exclude_terms: Option<&'a [String]>,
    page_concurrency: Option<usize>,
    navigation_timeout_ms: Option<u64>,
    overall_timeout_ms: Option<u64>,
    search_delay_ms: Option<u64>,
    retry_count: Option<usize>,
    user_agent: Option<&'a str>,
    fetch_pages: Option<bool>,
    cache_dir: Option<PathBuf>,
    cache_ttl_seconds: Option<u64>,
    timeout_seconds: Option<u64>,
    action_timeout: Option<Duration>,
}

struct PlaywrightPageAuditConfig<'a> {
    script_path: &'a Path,
    base_dir: &'a Path,
    screenshot_dir: Option<PathBuf>,
    viewports: Option<&'a [Value]>,
    navigation_timeout_ms: Option<u64>,
    max_text_chars: Option<usize>,
    timeout_seconds: Option<u64>,
    action_timeout: Option<Duration>,
}

fn call_web_fetch_tool(
    name: &str,
    input: &Value,
    config: WebFetchToolConfig<'_>,
) -> Result<Value, RuntimeError> {
    let url = required_input_string(name, input, "url")?;
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.url must start with http:// or https://"
        )));
    }
    let configured_timeout = Duration::from_secs(config.timeout_seconds.unwrap_or(30));
    let request_timeout = config
        .action_timeout
        .map(|timeout| timeout.min(configured_timeout))
        .unwrap_or(configured_timeout);
    let client = reqwest::blocking::Client::builder()
        .timeout(request_timeout)
        .build()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} HTTP client: {error}")))?;
    let mut request = client.get(url);
    for (header, value) in config.headers {
        request = request.header(header, render_json_template(value, input));
    }
    if let Some(env_name) = config.bearer_token_env {
        let token = std::env::var(env_name).map_err(|_| {
            RuntimeError::Provider(format!(
                "tool {name} requires bearer_token_env {env_name}, but it is not set"
            ))
        })?;
        request = request.bearer_auth(token);
    }

    let response = request
        .send()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} HTTP request: {error}")))?;
    let status = response.status().as_u16();
    let final_url = response.url().to_string();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = response
        .bytes()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} HTTP response: {error}")))?;
    let (text, truncated, bytes) = bytes_to_limited_text(&body, config.max_bytes);

    Ok(json!({
        "url": final_url.clone(),
        "status": status,
        "content_type": content_type.clone(),
        "text": text.clone(),
        "bytes": bytes,
        "truncated": truncated,
        "artifacts": [{
            "id": final_url.clone(),
            "kind": "web_page",
            "title": final_url.clone(),
            "uri": final_url.clone(),
            "content": text.clone(),
            "metadata": {
                "provider": "web_fetch",
                "status": status,
                "content_type": content_type.clone(),
                "bytes": bytes,
                "truncated": truncated
            }
        }],
    }))
}

fn call_playwright_page_audit_tool(
    name: &str,
    input: &Value,
    config: PlaywrightPageAuditConfig<'_>,
) -> Result<Value, RuntimeError> {
    let script = canonicalize_tool_path(name, "script_path", config.script_path)?;
    let base = canonicalize_tool_path(name, "base_dir", config.base_dir)?;
    let configured_timeout = Duration::from_secs(config.timeout_seconds.unwrap_or(30));
    let request_timeout = config
        .action_timeout
        .map(|timeout| timeout.min(configured_timeout))
        .unwrap_or(configured_timeout);

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

    let mut child = Command::new("node")
        .arg(&script)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| {
            RuntimeError::Provider(format!("tool {name} launch playwright page audit: {error}"))
        })?;
    {
        let mut stdin = child.stdin.take().ok_or_else(|| {
            RuntimeError::Provider(format!(
                "tool {name} playwright page audit stdin unavailable"
            ))
        })?;
        serde_json::to_writer(&mut stdin, &Value::Object(request)).map_err(|error| {
            RuntimeError::Provider(format!(
                "tool {name} write playwright page audit input: {error}"
            ))
        })?;
    }

    let started_at = std::time::Instant::now();
    loop {
        match child.try_wait().map_err(|error| {
            RuntimeError::Provider(format!("tool {name} playwright page audit wait: {error}"))
        })? {
            Some(status) => {
                let output = child.wait_with_output().map_err(|error| {
                    RuntimeError::Provider(format!(
                        "tool {name} playwright page audit collect output: {error}"
                    ))
                })?;
                if !status.success() {
                    return Err(RuntimeError::Provider(format!(
                        "tool {name} playwright page audit failed: {}",
                        provider_error_snippet(&String::from_utf8_lossy(&output.stderr))
                    )));
                }
                return serde_json::from_slice(&output.stdout).map_err(|error| {
                    RuntimeError::Provider(format!(
                        "tool {name} playwright page audit output was not valid JSON: {error}; body={}",
                        provider_error_snippet(&String::from_utf8_lossy(&output.stdout))
                    ))
                });
            }
            None if started_at.elapsed() >= request_timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RuntimeError::Provider(format!(
                    "tool {name} playwright page audit exceeded timeout_seconds={}",
                    request_timeout.as_secs()
                )));
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn call_playwright_search_tool(
    name: &str,
    input: &Value,
    script_path: &Path,
    config: PlaywrightSearchConfig<'_>,
) -> Result<Value, RuntimeError> {
    let query = required_input_string(name, input, "query")?;
    let script = canonicalize_tool_path(name, "script_path", script_path)?;
    let configured_timeout = Duration::from_secs(config.timeout_seconds.unwrap_or(120));
    let request_timeout = config
        .action_timeout
        .map(|timeout| timeout.min(configured_timeout))
        .unwrap_or(configured_timeout);
    let mut request = Map::new();
    request.insert("query".to_string(), Value::String(query.to_string()));
    request.insert(
        "max_results".to_string(),
        input
            .get("max_results")
            .cloned()
            .unwrap_or_else(|| Value::Number(config.max_results.unwrap_or(5).into())),
    );
    request.insert(
        "max_results_per_query".to_string(),
        input
            .get("max_results_per_query")
            .cloned()
            .unwrap_or_else(|| Value::Number(config.max_results_per_query.unwrap_or(10).into())),
    );
    request.insert(
        "max_per_domain".to_string(),
        input
            .get("max_per_domain")
            .cloned()
            .unwrap_or_else(|| Value::Number(config.max_per_domain.unwrap_or(2).into())),
    );
    request.insert(
        "max_content_chars".to_string(),
        input
            .get("max_content_chars")
            .cloned()
            .unwrap_or_else(|| Value::Number(config.max_content_chars.unwrap_or(12_000).into())),
    );
    request.insert(
        "page_concurrency".to_string(),
        input
            .get("page_concurrency")
            .cloned()
            .unwrap_or_else(|| Value::Number(config.page_concurrency.unwrap_or(3).into())),
    );
    request.insert(
        "navigation_timeout_ms".to_string(),
        input
            .get("navigation_timeout_ms")
            .cloned()
            .unwrap_or_else(|| {
                Value::Number(config.navigation_timeout_ms.unwrap_or(20_000).into())
            }),
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
    request.insert(
        "retry_count".to_string(),
        input
            .get("retry_count")
            .cloned()
            .unwrap_or_else(|| Value::Number(config.retry_count.unwrap_or(1).into())),
    );
    request.insert(
        "search_delay_ms".to_string(),
        input
            .get("search_delay_ms")
            .cloned()
            .unwrap_or_else(|| Value::Number(config.search_delay_ms.unwrap_or(0).into())),
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

    let mut child = Command::new("node")
        .arg(&script)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| {
            RuntimeError::Provider(format!("tool {name} launch playwright search: {error}"))
        })?;
    {
        let mut stdin = child.stdin.take().ok_or_else(|| {
            RuntimeError::Provider(format!("tool {name} playwright search stdin unavailable"))
        })?;
        serde_json::to_writer(&mut stdin, &Value::Object(request)).map_err(|error| {
            RuntimeError::Provider(format!(
                "tool {name} write playwright search input: {error}"
            ))
        })?;
    }

    let started_at = std::time::Instant::now();
    loop {
        match child.try_wait().map_err(|error| {
            RuntimeError::Provider(format!("tool {name} playwright search wait: {error}"))
        })? {
            Some(status) => {
                let output = child.wait_with_output().map_err(|error| {
                    RuntimeError::Provider(format!(
                        "tool {name} playwright search collect output: {error}"
                    ))
                })?;
                if !status.success() {
                    return Err(RuntimeError::Provider(format!(
                        "tool {name} playwright search failed: {}",
                        provider_error_snippet(&String::from_utf8_lossy(&output.stderr))
                    )));
                }
                return serde_json::from_slice(&output.stdout).map_err(|error| {
                    RuntimeError::Provider(format!(
                        "tool {name} playwright search output was not valid JSON: {error}; body={}",
                        provider_error_snippet(&String::from_utf8_lossy(&output.stdout))
                    ))
                });
            }
            None if started_at.elapsed() >= request_timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RuntimeError::Provider(format!(
                    "tool {name} playwright search exceeded timeout_seconds={}",
                    request_timeout.as_secs()
                )));
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn insert_input_or_config_array(
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

fn call_file_read_tool(
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

fn call_file_read_many_tool(
    name: &str,
    input: &Value,
    base_dir: &Path,
    max_bytes: usize,
    max_files: usize,
) -> Result<Value, RuntimeError> {
    let entries = file_read_many_entries(name, input)?;
    if entries.is_empty() {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.files must contain at least one file"
        )));
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

fn numbered_content(content: &str, first_line: usize) -> String {
    content
        .lines()
        .enumerate()
        .map(|(index, line)| format!("{:05}| {}", first_line + index, line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_likely_binary(bytes: &[u8]) -> bool {
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

fn file_modified_time(name: &str, label: &str, path: &Path) -> Result<SystemTime, RuntimeError> {
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
    Exact,
    LineTrimmed,
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
            "exact" => Ok(Self::Exact),
            "line_trimmed" => Ok(Self::LineTrimmed),
            "indentation_flexible" => Ok(Self::IndentationFlexible),
            _ => Err(RuntimeError::Provider(format!(
                "tool {tool_name} {label}.match_strategy must be one of exact, line_trimmed, indentation_flexible"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::LineTrimmed => "line_trimmed",
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
) -> Vec<EditMatch> {
    match strategy {
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

struct FileWriteOptions<'a> {
    base_dir: &'a Path,
    max_bytes: usize,
    create_dirs: bool,
    allow_overwrite: bool,
    require_read: bool,
    read_snapshots: &'a BTreeMap<PathBuf, SystemTime>,
}

fn call_file_write_tool(
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

fn call_file_edit_tool(
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
        let matches = find_edit_matches(&updated, operation.old_string, operation.match_strategy);
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
                operation.match_strategy.as_str()
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
        strategies.push(operation.match_strategy.as_str());
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
    fs::write(&path, updated_bytes)
        .map_err(|error| RuntimeError::Provider(format!("tool {name} write file: {error}")))?;

    Ok(json!({
        "path": path.display().to_string(),
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

struct FilePatchOptions<'a> {
    repo_dir: &'a Path,
    max_bytes: usize,
    max_files: usize,
    require_read: bool,
    allow_new_files: bool,
    allow_delete_files: bool,
    read_snapshots: &'a BTreeMap<PathBuf, SystemTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatchPathKind {
    Existing,
    New,
    Delete,
}

fn call_file_patch_tool(
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

fn call_git_diff_tool(
    name: &str,
    input: &Value,
    repo_dir: &Path,
    max_bytes: usize,
) -> Result<Value, RuntimeError> {
    let repo = canonicalize_tool_path(name, "repo_dir", repo_dir)?;
    let mut command = Command::new("git");
    command.arg("-C").arg(&repo).arg("diff");
    if input
        .get("staged")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        command.arg("--staged");
    }
    let paths = git_diff_paths(name, input)?;
    if !paths.is_empty() {
        command.arg("--");
        command.args(&paths);
    }
    let output = command
        .output()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} git diff: {error}")))?;
    if !output.status.success() {
        return Err(RuntimeError::Provider(format!(
            "tool {name} git diff failed: {}",
            provider_error_snippet(&String::from_utf8_lossy(&output.stderr))
        )));
    }
    let (diff, truncated, bytes) = bytes_to_limited_text(&output.stdout, max_bytes);
    let artifact_id = format!(
        "git-diff:{}:{}:{}",
        repo.display(),
        input
            .get("staged")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        paths.join(",")
    );
    Ok(json!({
        "repo": repo.display().to_string(),
        "diff": diff.clone(),
        "bytes": bytes,
        "truncated": truncated,
        "artifacts": [{
            "id": artifact_id.clone(),
            "kind": "git_diff",
            "title": "git diff",
            "uri": repo.display().to_string(),
            "content": diff.clone(),
            "metadata": {
                "provider": "git_diff",
                "repo": repo.display().to_string(),
                "paths": paths.clone(),
                "bytes": bytes,
                "truncated": truncated
            }
        }],
    }))
}

fn call_git_status_tool(
    name: &str,
    repo_dir: &Path,
    max_files: usize,
) -> Result<Value, RuntimeError> {
    let repo = canonicalize_tool_path(name, "repo_dir", repo_dir)?;
    let output = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["status", "--porcelain=v1", "--untracked-files=all"])
        .output()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} git status: {error}")))?;
    if !output.status.success() {
        return Err(RuntimeError::Provider(format!(
            "tool {name} git status failed: {}",
            provider_error_snippet(&String::from_utf8_lossy(&output.stderr))
        )));
    }
    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    let mut entries = Vec::new();
    for line in raw.lines().take(max_files) {
        if let Some(entry) = parse_git_status_line(line) {
            entries.push(entry);
        }
    }
    let truncated = raw.lines().count() > entries.len();
    let rendered = entries
        .iter()
        .map(|entry| {
            format!(
                "{}{} {}",
                entry["index"].as_str().unwrap_or_default(),
                entry["worktree"].as_str().unwrap_or_default(),
                entry["path"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(json!({
        "repo": repo.display().to_string(),
        "clean": entries.is_empty(),
        "entries": entries,
        "file_count": entries.len(),
        "truncated": truncated,
        "artifacts": [{
            "id": format!("git-status:{}", repo.display()),
            "kind": "git_status",
            "title": "git status",
            "uri": repo.display().to_string(),
            "content": rendered,
            "metadata": {
                "provider": "git_status",
                "repo": repo.display().to_string(),
                "file_count": entries.len(),
                "truncated": truncated
            }
        }]
    }))
}

fn parse_git_status_line(line: &str) -> Option<Value> {
    if line.len() < 4 {
        return None;
    }
    let index = line.chars().next()?.to_string();
    let worktree = line.chars().nth(1)?.to_string();
    let rest = line.get(3..)?;
    let (path, original_path) = rest
        .split_once(" -> ")
        .map(|(from, to)| (to.to_string(), Some(from.to_string())))
        .unwrap_or_else(|| (rest.to_string(), None));
    Some(json!({
        "index": index,
        "worktree": worktree,
        "path": path,
        "original_path": original_path,
        "status": git_status_label(line.get(..2).unwrap_or_default())
    }))
}

fn git_status_label(code: &str) -> &'static str {
    match code {
        "??" => "untracked",
        "!!" => "ignored",
        " M" | "M " | "MM" => "modified",
        " A" | "A " | "AM" => "added",
        " D" | "D " => "deleted",
        "R " | " R" => "renamed",
        "C " | " C" => "copied",
        "UU" | "AA" | "DD" | "AU" | "UA" | "DU" | "UD" => "unmerged",
        _ => "changed",
    }
}

fn call_repo_files_tool(
    name: &str,
    input: &Value,
    repo_dir: &Path,
    max_files: usize,
) -> Result<Value, RuntimeError> {
    let repo = canonicalize_tool_path(name, "repo_dir", repo_dir)?;
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let paths = repo_tool_paths(name, input)?;
    let mut command = Command::new("rg");
    command.arg("--files");
    if let Some(glob) = input.get("glob").and_then(Value::as_str) {
        validate_git_pathspec(name, glob)?;
        command.arg("-g").arg(glob);
    }
    if !paths.is_empty() {
        command.args(&paths);
    }
    let output = command
        .current_dir(&repo)
        .output()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} repo files: {error}")))?;
    if !output.status.success() {
        return Err(RuntimeError::Provider(format!(
            "tool {name} repo files failed: {}",
            provider_error_snippet(&String::from_utf8_lossy(&output.stderr))
        )));
    }
    let mut files = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let path = line.trim();
        if path.is_empty() {
            continue;
        }
        if !query.is_empty() && !path.to_lowercase().contains(&query) {
            continue;
        }
        files.push(path.to_string());
        if files.len() >= max_files {
            break;
        }
    }
    let content = files.join("\n");
    Ok(json!({
        "repo": repo.display().to_string(),
        "query": query,
        "files": files,
        "truncated": files.len() >= max_files,
        "artifacts": [{
            "id": format!("repo-files:{}:{}", repo.display(), input.get("query").and_then(Value::as_str).unwrap_or_default()),
            "kind": "repo_listing",
            "title": "repo files",
            "uri": repo.display().to_string(),
            "content": content,
            "metadata": {
                "provider": "repo_files",
                "repo": repo.display().to_string(),
                "max_files": max_files
            }
        }]
    }))
}

fn call_repo_search_tool(
    name: &str,
    input: &Value,
    repo_dir: &Path,
    max_matches: usize,
    max_bytes: usize,
) -> Result<Value, RuntimeError> {
    let query = required_input_string(name, input, "query")?;
    let mode = optional_string_input(name, input, "mode")?.unwrap_or("fixed");
    if !matches!(mode, "fixed" | "regex") {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.mode must be fixed or regex"
        )));
    }
    let effective_max_matches =
        optional_bounded_usize_input(name, input, "max_matches", max_matches)?
            .unwrap_or(max_matches);
    let repo = canonicalize_tool_path(name, "repo_dir", repo_dir)?;
    let paths = repo_tool_paths(name, input)?;
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
    command.arg(query);
    if let Some(glob) = input.get("glob").and_then(Value::as_str) {
        validate_git_pathspec(name, glob)?;
        command.arg("-g").arg(glob);
    }
    if !paths.is_empty() {
        command.args(&paths);
    }
    let output = command
        .current_dir(&repo)
        .output()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} repo search: {error}")))?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(RuntimeError::Provider(format!(
            "tool {name} repo search failed: {}",
            provider_error_snippet(&String::from_utf8_lossy(&output.stderr))
        )));
    }
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
    let truncated = raw.lines().count() > matches.len() || truncated_bytes;
    Ok(json!({
        "repo": repo.display().to_string(),
        "query": query,
        "mode": mode,
        "matches": matches,
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
                "mode": mode,
                "paths": paths,
                "max_matches": effective_max_matches,
                "bytes": bytes,
                "truncated": truncated
            }
        }]
    }))
}

fn call_repo_context_tool(
    name: &str,
    input: &Value,
    repo_dir: &Path,
    max_matches: usize,
    max_files: usize,
    context_lines: usize,
    max_bytes: usize,
) -> Result<Value, RuntimeError> {
    let query = required_input_string(name, input, "query")?;
    let mode = optional_string_input(name, input, "mode")?.unwrap_or("fixed");
    if !matches!(mode, "fixed" | "regex") {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.mode must be fixed or regex"
        )));
    }
    let repo = canonicalize_tool_path(name, "repo_dir", repo_dir)?;
    let paths = repo_tool_paths(name, input)?;
    let effective_max_matches =
        optional_bounded_usize_input(name, input, "max_matches", max_matches)?
            .unwrap_or(max_matches);
    let effective_max_files =
        optional_bounded_usize_input(name, input, "max_files", max_files)?.unwrap_or(max_files);
    let effective_context_lines =
        optional_bounded_usize_input(name, input, "context_lines", context_lines)?
            .unwrap_or(context_lines);

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
    command.arg(query);
    if let Some(glob) = input.get("glob").and_then(Value::as_str) {
        validate_git_pathspec(name, glob)?;
        command.arg("-g").arg(glob);
    }
    if !paths.is_empty() {
        command.args(&paths);
    }
    let output = command
        .current_dir(&repo)
        .output()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} repo context: {error}")))?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(RuntimeError::Provider(format!(
            "tool {name} repo context failed: {}",
            provider_error_snippet(&String::from_utf8_lossy(&output.stderr))
        )));
    }

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
    Ok(json!({
        "repo": repo.display().to_string(),
        "query": query,
        "mode": mode,
        "matches": matches,
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
                "mode": mode,
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

fn call_repo_symbols_tool(
    name: &str,
    input: &Value,
    repo_dir: &Path,
    max_symbols: usize,
    max_bytes: usize,
) -> Result<Value, RuntimeError> {
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let effective_max_symbols =
        optional_bounded_usize_input(name, input, "max_symbols", max_symbols)?
            .unwrap_or(max_symbols);
    let repo = canonicalize_tool_path(name, "repo_dir", repo_dir)?;
    let paths = repo_tool_paths(name, input)?;
    let pattern = r"^\s*(pub\s+|export\s+|async\s+|static\s+|final\s+|private\s+|protected\s+|public\s+)*(fn|function|def|class|struct|enum|trait|interface|type|const|let|var)\s+[A-Za-z_$][A-Za-z0-9_$]*";
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
    if let Some(glob) = input.get("glob").and_then(Value::as_str) {
        validate_git_pathspec(name, glob)?;
        command.arg("-g").arg(glob);
    }
    if !paths.is_empty() {
        command.args(&paths);
    }
    let output = command
        .current_dir(&repo)
        .output()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} repo symbols: {error}")))?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(RuntimeError::Provider(format!(
            "tool {name} repo symbols failed: {}",
            provider_error_snippet(&String::from_utf8_lossy(&output.stderr))
        )));
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let mut symbols = Vec::new();
    for line in raw.lines() {
        let item = parse_rg_vimgrep_line(line);
        let text = item["text"].as_str().unwrap_or_default();
        let Some((kind, symbol_name)) = parse_symbol_declaration(text) else {
            continue;
        };
        if !query.is_empty()
            && !symbol_name.to_lowercase().contains(&query)
            && !item["path"]
                .as_str()
                .unwrap_or_default()
                .to_lowercase()
                .contains(&query)
        {
            continue;
        }
        symbols.push(json!({
            "path": item["path"],
            "line": item["line"],
            "column": item["column"],
            "kind": kind,
            "name": symbol_name,
            "text": text.trim()
        }));
        if symbols.len() >= effective_max_symbols {
            break;
        }
    }
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
    let (content, truncated_bytes, bytes) = bytes_to_limited_text(rendered.as_bytes(), max_bytes);
    let truncated = raw.lines().count() > symbols.len() || truncated_bytes;
    Ok(json!({
        "repo": repo.display().to_string(),
        "query": query,
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
                "query": query,
                "paths": paths,
                "max_symbols": effective_max_symbols,
                "bytes": bytes,
                "truncated": truncated
            }
        }]
    }))
}

fn parse_symbol_declaration(line: &str) -> Option<(&'static str, String)> {
    let mut tokens = line
        .trim_start()
        .split(|character: char| character.is_whitespace() || character == '(' || character == '<')
        .filter(|token| !token.is_empty());
    let mut token = tokens.next()?;
    while matches!(
        token,
        "pub" | "export" | "async" | "static" | "final" | "private" | "protected" | "public"
    ) {
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

fn call_repo_references_tool(
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
    let paths = repo_tool_paths(name, input)?;
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
        validate_git_pathspec(name, glob)?;
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
        let definition_kind = parse_symbol_declaration(text)
            .and_then(|(kind, declaration)| (declaration == symbol).then_some(kind));
        let reference = json!({
            "path": item["path"],
            "line": item["line"],
            "column": item["column"],
            "text": text,
            "definition": definition_kind.is_some(),
            "kind": definition_kind.unwrap_or("reference")
        });
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

fn is_identifier_like(symbol: &str) -> bool {
    let mut chars = symbol.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_' || first == '$') {
        return false;
    }
    chars.all(|character| character.is_ascii_alphanumeric() || character == '_' || character == '$')
}

fn contains_identifier_token(line: &str, symbol: &str) -> bool {
    line.match_indices(symbol).any(|(index, _)| {
        let before = line[..index].chars().next_back();
        let after = line[index + symbol.len()..].chars().next();
        !before.is_some_and(is_identifier_character) && !after.is_some_and(is_identifier_character)
    })
}

fn is_identifier_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_' || character == '$'
}

fn call_diagnostic_context_tool(
    name: &str,
    input: &Value,
    repo_dir: &Path,
    max_diagnostics: usize,
    context_lines: usize,
    max_bytes: usize,
) -> Result<Value, RuntimeError> {
    let diagnostics = input
        .get("diagnostics")
        .ok_or_else(|| {
            RuntimeError::Provider(format!("tool {name} input.diagnostics is required"))
        })?
        .as_array()
        .ok_or_else(|| {
            RuntimeError::Provider(format!("tool {name} input.diagnostics must be an array"))
        })?;
    let effective_max_diagnostics =
        optional_bounded_usize_input(name, input, "max_diagnostics", max_diagnostics)?
            .unwrap_or(max_diagnostics);
    let effective_context_lines =
        optional_bounded_usize_input(name, input, "context_lines", context_lines)?
            .unwrap_or(context_lines);
    let repo = canonicalize_tool_path(name, "repo_dir", repo_dir)?;
    let mut files: BTreeMap<PathBuf, DiagnosticContextFile> = BTreeMap::new();
    let mut considered = Vec::new();
    let mut unreadable = Vec::new();

    for (index, diagnostic) in diagnostics
        .iter()
        .take(effective_max_diagnostics)
        .enumerate()
    {
        let path = diagnostic.get("path").and_then(Value::as_str);
        let line = diagnostic.get("line").and_then(Value::as_u64);
        let Some(path) = path else {
            unreadable.push(json!({
                "diagnostic_index": index,
                "reason": "missing path"
            }));
            continue;
        };
        let Some(line) = line
            .and_then(|line| usize::try_from(line).ok())
            .filter(|line| *line > 0)
        else {
            unreadable.push(json!({
                "diagnostic_index": index,
                "path": path,
                "reason": "missing positive line"
            }));
            continue;
        };
        let Some((absolute_path, relative_path)) = resolve_diagnostic_context_path(&repo, path)
        else {
            unreadable.push(json!({
                "diagnostic_index": index,
                "path": path,
                "reason": "path is outside repo_dir or cannot be read"
            }));
            continue;
        };
        let body = match fs::read(&absolute_path) {
            Ok(body) => body,
            Err(error) => {
                unreadable.push(json!({
                    "diagnostic_index": index,
                    "path": relative_path,
                    "reason": format!("read failed: {error}")
                }));
                continue;
            }
        };
        if is_likely_binary(&body) {
            unreadable.push(json!({
                "diagnostic_index": index,
                "path": relative_path,
                "reason": "file appears to be binary"
            }));
            continue;
        }
        let content = match std::str::from_utf8(&body) {
            Ok(content) => content.to_string(),
            Err(_) => {
                unreadable.push(json!({
                    "diagnostic_index": index,
                    "path": relative_path,
                    "reason": "file is not valid UTF-8"
                }));
                continue;
            }
        };
        let total_lines = content.lines().count().max(1);
        if line > total_lines {
            unreadable.push(json!({
                "diagnostic_index": index,
                "path": relative_path,
                "line": line,
                "reason": "line is outside file"
            }));
            continue;
        }
        considered.push(diagnostic.clone());
        let file = files
            .entry(absolute_path)
            .or_insert_with(|| DiagnosticContextFile {
                relative_path,
                content,
                total_lines,
                lines: Vec::new(),
                diagnostic_indexes: Vec::new(),
            });
        file.lines.push(line);
        file.diagnostic_indexes.push(index);
    }

    let mut snippets = Vec::new();
    let mut rendered = String::new();
    let mut truncated = diagnostics.len() > effective_max_diagnostics;
    for file in files.values() {
        for (start_line, end_line) in
            merge_line_ranges(&file.lines, file.total_lines, effective_context_lines)
        {
            let content = numbered_line_range(&file.content, start_line, end_line);
            let remaining = max_bytes.saturating_sub(rendered.len());
            if remaining == 0 {
                truncated = true;
                break;
            }
            let (limited_content, content_truncated, _source_bytes) =
                bytes_to_limited_text(content.as_bytes(), remaining);
            let diagnostic_indexes = file
                .lines
                .iter()
                .zip(file.diagnostic_indexes.iter())
                .filter_map(|(line, index)| {
                    (*line >= start_line && *line <= end_line).then_some(*index)
                })
                .collect::<Vec<_>>();
            truncated |= content_truncated;
            if !rendered.is_empty() {
                rendered.push_str("\n\n");
            }
            rendered.push_str(&format!(
                "== {}:{}-{} ==\n{}",
                file.relative_path, start_line, end_line, limited_content
            ));
            snippets.push(json!({
                "path": file.relative_path,
                "start_line": start_line,
                "end_line": end_line,
                "content": limited_content,
                "diagnostic_indexes": diagnostic_indexes,
                "truncated": content_truncated
            }));
            if content_truncated {
                break;
            }
        }
        if rendered.len() >= max_bytes {
            truncated = true;
            break;
        }
    }
    let bytes = rendered.len();

    Ok(json!({
        "repo": repo.display().to_string(),
        "diagnostics": considered,
        "snippets": snippets,
        "unreadable": unreadable,
        "diagnostic_count": diagnostics.len(),
        "max_diagnostics": effective_max_diagnostics,
        "context_lines": effective_context_lines,
        "bytes": bytes,
        "truncated": truncated,
        "artifacts": [{
            "id": format!("diagnostic-context:{}", repo.display()),
            "kind": "diagnostic_context",
            "title": "diagnostic context",
            "uri": repo.display().to_string(),
            "content": rendered,
            "metadata": {
                "provider": "diagnostic_context",
                "repo": repo.display().to_string(),
                "diagnostic_count": diagnostics.len(),
                "snippet_count": snippets.len(),
                "unreadable_count": unreadable.len(),
                "max_diagnostics": effective_max_diagnostics,
                "context_lines": effective_context_lines,
                "bytes": bytes,
                "truncated": truncated
            }
        }]
    }))
}

#[derive(Debug)]
struct DiagnosticContextFile {
    relative_path: String,
    content: String,
    total_lines: usize,
    lines: Vec<usize>,
    diagnostic_indexes: Vec<usize>,
}

fn resolve_diagnostic_context_path(
    repo: &Path,
    diagnostic_path: &str,
) -> Option<(PathBuf, String)> {
    let raw_path = Path::new(diagnostic_path);
    if !raw_path.is_absolute()
        && validate_git_pathspec("diagnostic.context", diagnostic_path).is_err()
    {
        return None;
    }
    let candidate = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        repo.join(raw_path)
    };
    let absolute = candidate.canonicalize().ok()?;
    if !absolute.starts_with(repo) {
        return None;
    }
    let relative = absolute
        .strip_prefix(repo)
        .ok()?
        .to_string_lossy()
        .to_string();
    Some((absolute, relative))
}

fn call_command_run_tool(
    name: &str,
    input: &Value,
    cwd: &Path,
    commands: &BTreeMap<String, Vec<String>>,
    timeout_seconds: u64,
    max_bytes: usize,
) -> Result<Value, RuntimeError> {
    let command_name = required_input_string(name, input, "command")?;
    let Some(command) = commands.get(command_name) else {
        return Err(RuntimeError::Provider(format!(
            "tool {name} command {command_name} is not configured"
        )));
    };
    let cwd = canonicalize_tool_path(name, "cwd", cwd)?;
    let (program, args) = command.split_first().ok_or_else(|| {
        RuntimeError::Provider(format!("tool {name} command {command_name} is empty"))
    })?;
    let mut child = Command::new(program)
        .args(args)
        .current_dir(&cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} command run: {error}")))?;
    let timeout = Duration::from_secs(timeout_seconds);
    let started_at = std::time::Instant::now();
    loop {
        match child
            .try_wait()
            .map_err(|error| RuntimeError::Provider(format!("tool {name} command wait: {error}")))?
        {
            Some(status) => {
                let output = child.wait_with_output().map_err(|error| {
                    RuntimeError::Provider(format!("tool {name} command output: {error}"))
                })?;
                let mut combined = Vec::new();
                combined.extend_from_slice(&output.stdout);
                if !output.stderr.is_empty() {
                    combined.extend_from_slice(b"\n[stderr]\n");
                    combined.extend_from_slice(&output.stderr);
                }
                let combined_text = String::from_utf8_lossy(&combined).to_string();
                let diagnostics = extract_command_diagnostics(&combined_text, 50);
                let diagnostics_count = diagnostics.len();
                let (log, truncated, bytes) = bytes_to_limited_text(&combined, max_bytes);
                return Ok(json!({
                    "command": command_name,
                    "argv": command,
                    "cwd": cwd.display().to_string(),
                    "status": status.code(),
                    "success": status.success(),
                    "log": log.clone(),
                    "diagnostics": diagnostics,
                    "bytes": bytes,
                    "truncated": truncated,
                    "artifacts": [{
                        "id": format!("command-run:{}:{}", cwd.display(), command_name),
                        "kind": "test_log",
                        "title": format!("command run: {command_name}"),
                        "uri": cwd.display().to_string(),
                        "content": log,
                        "metadata": {
                            "provider": "command_run",
                            "command": command_name,
                            "argv": command,
                            "cwd": cwd.display().to_string(),
                            "status": status.code(),
                            "success": status.success(),
                            "diagnostics_count": diagnostics_count,
                            "bytes": bytes,
                            "truncated": truncated
                        }
                    }]
                }));
            }
            None if started_at.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RuntimeError::Provider(format!(
                    "tool {name} command {command_name} exceeded timeout_seconds={timeout_seconds}"
                )));
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn extract_command_diagnostics(log: &str, max_diagnostics: usize) -> Vec<Value> {
    let mut diagnostics = Vec::new();
    let mut pending_rust = None;
    let mut pending_python_frames: Vec<(String, u64, String)> = Vec::new();
    let mut pending_diagnostic_file: Option<String> = None;
    for line in log.lines() {
        let trimmed = line.trim_start();
        if let Some((severity, message)) = parse_rust_severity_message(trimmed) {
            pending_rust = Some((severity, message, trimmed.to_string()));
            continue;
        }
        if let Some(path) = parse_diagnostic_file_context(line) {
            pending_diagnostic_file = Some(path);
            continue;
        }
        if let Some(path) = pending_diagnostic_file.as_deref() {
            if let Some(diagnostic) = parse_indented_line_column_diagnostic(line, path) {
                diagnostics.push(diagnostic);
                pending_rust = None;
                pending_python_frames.clear();
            }
        }
        if let Some((path, line_number)) = parse_python_traceback_location(trimmed) {
            pending_python_frames.push((path, line_number, trimmed.to_string()));
            continue;
        }
        if let Some(message) = parse_python_exception_line(trimmed) {
            if let Some((path, line_number, raw)) = pending_python_frames.last() {
                diagnostics.push(json!({
                    "source": "command_run",
                    "severity": "error",
                    "path": path,
                    "line": line_number,
                    "column": 1,
                    "message": message,
                    "raw": raw
                }));
                pending_python_frames.clear();
                pending_rust = None;
            }
        }
        if let Some((path, line_number, column)) = parse_rust_location_line(trimmed) {
            if let Some((severity, message, raw)) = pending_rust.take() {
                diagnostics.push(json!({
                    "source": "command_run",
                    "severity": severity,
                    "path": path,
                    "line": line_number,
                    "column": column,
                    "message": message,
                    "raw": raw
                }));
            }
        } else if let Some(diagnostic) = parse_typescript_diagnostic(trimmed)
            .or_else(|| parse_colon_diagnostic(trimmed))
            .or_else(|| parse_colon_line_diagnostic(trimmed))
        {
            diagnostics.push(diagnostic);
            pending_rust = None;
            pending_python_frames.clear();
            pending_diagnostic_file = None;
        }
        if diagnostics.len() >= max_diagnostics {
            break;
        }
    }
    diagnostics
}

fn parse_rust_severity_message(line: &str) -> Option<(String, String)> {
    for severity in ["error", "warning"] {
        if line == severity
            || line.starts_with(&format!("{severity}:"))
            || line.starts_with(&format!("{severity}["))
        {
            let message = line
                .split_once(':')
                .map(|(_, message)| message.trim())
                .filter(|message| !message.is_empty())
                .unwrap_or(line)
                .to_string();
            return Some((severity.to_string(), message));
        }
    }
    None
}

fn parse_rust_location_line(line: &str) -> Option<(String, u64, u64)> {
    let rest = line.strip_prefix("-->")?.trim();
    parse_location(rest)
}

fn parse_typescript_diagnostic(line: &str) -> Option<Value> {
    let open = line.find('(')?;
    let close = line[open + 1..].find(')')? + open + 1;
    let path = line[..open].trim();
    let mut location = line[open + 1..close].split(',');
    let line_number = location.next()?.trim().parse::<u64>().ok()?;
    let column = location.next()?.trim().parse::<u64>().ok()?;
    let rest = line[close + 1..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let (severity, message) = split_severity_message(rest)?;
    Some(json!({
        "source": "command_run",
        "severity": severity,
        "path": path,
        "line": line_number,
        "column": column,
        "message": message,
        "raw": line
    }))
}

fn parse_python_traceback_location(line: &str) -> Option<(String, u64)> {
    let rest = line.strip_prefix("File ")?;
    let rest = rest.strip_prefix('"')?;
    let (path, rest) = rest.split_once('"')?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(", line ")?;
    let line_number = rest
        .split(|character: char| !character.is_ascii_digit())
        .next()?
        .parse::<u64>()
        .ok()?;
    Some((path.to_string(), line_number))
}

fn parse_python_exception_line(line: &str) -> Option<String> {
    if line.is_empty()
        || line == "Traceback (most recent call last):"
        || line.starts_with("File ")
        || line.starts_with("During handling of the above exception")
        || line.starts_with("The above exception was the direct cause")
    {
        return None;
    }
    let (exception, _message) = line.split_once(':').unwrap_or((line, ""));
    let exception = exception.trim();
    let valid_exception_name = exception
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '_' || character == '.')
        && exception
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_uppercase() || character == '_');
    valid_exception_name.then(|| line.to_string())
}

fn parse_colon_diagnostic(line: &str) -> Option<Value> {
    let parts = line.split(':').collect::<Vec<_>>();
    if parts.len() < 4 {
        return None;
    }
    for index in 1..parts.len().saturating_sub(2) {
        let Ok(line_number) = parts[index].trim().parse::<u64>() else {
            continue;
        };
        let Ok(column) = parts[index + 1].trim().parse::<u64>() else {
            continue;
        };
        let rest = parts[index + 2..].join(":");
        let Some((severity, message)) = split_severity_message(rest.trim()) else {
            continue;
        };
        let path = parts[..index].join(":");
        return Some(json!({
            "source": "command_run",
            "severity": severity,
            "path": path,
            "line": line_number,
            "column": column,
            "message": message,
            "raw": line
        }));
    }
    None
}

fn parse_colon_line_diagnostic(line: &str) -> Option<Value> {
    let parts = line.split(':').collect::<Vec<_>>();
    if parts.len() < 3 {
        return None;
    }
    for index in 1..parts.len().saturating_sub(1) {
        let Ok(line_number) = parts[index].trim().parse::<u64>() else {
            continue;
        };
        let rest = parts[index + 1..].join(":");
        let rest = rest.trim();
        if rest.is_empty() {
            continue;
        }
        let path = parts[..index].join(":");
        if path.trim().is_empty() {
            continue;
        }
        let (severity, message) = if let Some((severity, message)) = split_severity_message(rest) {
            (severity, message)
        } else if let Some(message) = parse_python_exception_line(rest) {
            ("error".to_string(), message)
        } else {
            continue;
        };
        return Some(json!({
            "source": "command_run",
            "severity": severity,
            "path": path,
            "line": line_number,
            "column": 1,
            "message": message,
            "raw": line
        }));
    }
    None
}

fn parse_diagnostic_file_context(line: &str) -> Option<String> {
    let path = line.trim();
    if path.is_empty()
        || path.contains(char::is_whitespace)
        || path.contains(':')
        || !path.contains('.')
        || path.starts_with(">")
    {
        return None;
    }
    Some(path.to_string())
}

fn parse_indented_line_column_diagnostic(line: &str, path: &str) -> Option<Value> {
    if !line.starts_with(char::is_whitespace) {
        return None;
    }
    let trimmed = line.trim_start();
    let (line_text, rest) = trimmed.split_once(':')?;
    let line_number = line_text.trim().parse::<u64>().ok()?;
    let (column_text, rest) = rest.trim_start().split_once(char::is_whitespace)?;
    let column = column_text.trim().parse::<u64>().ok()?;
    let rest = rest.trim_start();
    let (severity, message) = split_severity_message(rest)?;
    Some(json!({
        "source": "command_run",
        "severity": severity,
        "path": path,
        "line": line_number,
        "column": column,
        "message": message,
        "raw": line.trim()
    }))
}

fn split_severity_message(text: &str) -> Option<(String, String)> {
    for severity in ["error", "warning"] {
        if text == severity {
            return Some((severity.to_string(), String::new()));
        }
        if let Some(message) = text.strip_prefix(&format!("{severity}:")) {
            return Some((severity.to_string(), message.trim().to_string()));
        }
        if text.starts_with(&format!("{severity} ")) {
            let message = text[severity.len()..].trim_start().to_string();
            return Some((severity.to_string(), message));
        }
    }
    None
}

fn parse_location(text: &str) -> Option<(String, u64, u64)> {
    let parts = text.split(':').collect::<Vec<_>>();
    if parts.len() < 3 {
        return None;
    }
    let column = parts.last()?.trim().parse::<u64>().ok()?;
    let line_number = parts.get(parts.len() - 2)?.trim().parse::<u64>().ok()?;
    let path = parts[..parts.len() - 2].join(":");
    Some((path, line_number, column))
}

pub fn provider_error_snippet(body: &str) -> String {
    sanitize_trace_text(
        body,
        &TraceWriteOptions {
            redact_sensitive: true,
            max_string_chars: Some(2048),
            max_event_bytes: None,
        },
    )
}

fn required_input_string<'a>(
    tool_name: &str,
    input: &'a Value,
    field: &str,
) -> Result<&'a str, RuntimeError> {
    input.get(field).and_then(Value::as_str).ok_or_else(|| {
        RuntimeError::Provider(format!("tool {tool_name} input.{field} must be a string"))
    })
}

fn required_labeled_string_input<'a>(
    tool_name: &str,
    input: &'a Value,
    label: &str,
    field: &str,
) -> Result<&'a str, RuntimeError> {
    input.get(field).and_then(Value::as_str).ok_or_else(|| {
        RuntimeError::Provider(format!("tool {tool_name} {label}.{field} must be a string"))
    })
}

fn optional_string_input<'a>(
    tool_name: &str,
    input: &'a Value,
    field: &str,
) -> Result<Option<&'a str>, RuntimeError> {
    let Some(value) = input.get(field) else {
        return Ok(None);
    };
    value.as_str().map(Some).ok_or_else(|| {
        RuntimeError::Provider(format!("tool {tool_name} input.{field} must be a string"))
    })
}

fn optional_labeled_string_input<'a>(
    tool_name: &str,
    input: &'a Value,
    label: &str,
    field: &str,
) -> Result<Option<&'a str>, RuntimeError> {
    let Some(value) = input.get(field) else {
        return Ok(None);
    };
    value.as_str().map(Some).ok_or_else(|| {
        RuntimeError::Provider(format!("tool {tool_name} {label}.{field} must be a string"))
    })
}

fn optional_positive_usize_input(
    tool_name: &str,
    input: &Value,
    field: &str,
) -> Result<Option<usize>, RuntimeError> {
    let Some(value) = input.get(field) else {
        return Ok(None);
    };
    let Some(number) = value.as_u64() else {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.{field} must be a positive integer"
        )));
    };
    if number == 0 {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.{field} must be greater than 0"
        )));
    }
    usize::try_from(number)
        .map(Some)
        .map_err(|_| RuntimeError::Provider(format!("tool {tool_name} input.{field} is too large")))
}

fn optional_bounded_usize_input(
    tool_name: &str,
    input: &Value,
    field: &str,
    configured_max: usize,
) -> Result<Option<usize>, RuntimeError> {
    let Some(value) = optional_positive_usize_input(tool_name, input, field)? else {
        return Ok(None);
    };
    Ok(Some(value.min(configured_max)))
}

fn optional_threshold_percent_input(
    tool_name: &str,
    input: &Value,
    field: &str,
) -> Result<Option<u64>, RuntimeError> {
    let Some(value) = input.get(field) else {
        return Ok(None);
    };
    let Some(number) = value.as_u64() else {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.{field} must be a positive integer"
        )));
    };
    if number == 0 || number > 100 {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.{field} must be between 1 and 100"
        )));
    }
    Ok(Some(number))
}

fn optional_bool_input(
    tool_name: &str,
    input: &Value,
    field: &str,
) -> Result<Option<bool>, RuntimeError> {
    let Some(value) = input.get(field) else {
        return Ok(None);
    };
    value.as_bool().map(Some).ok_or_else(|| {
        RuntimeError::Provider(format!("tool {tool_name} input.{field} must be a boolean"))
    })
}

fn optional_labeled_bool_input(
    tool_name: &str,
    input: &Value,
    label: &str,
    field: &str,
) -> Result<Option<bool>, RuntimeError> {
    let Some(value) = input.get(field) else {
        return Ok(None);
    };
    value.as_bool().map(Some).ok_or_else(|| {
        RuntimeError::Provider(format!(
            "tool {tool_name} {label}.{field} must be a boolean"
        ))
    })
}

fn select_line_range(content: &str, start_line: Option<usize>, end_line: Option<usize>) -> String {
    if start_line.is_none() && end_line.is_none() {
        return content.to_string();
    }
    let start = start_line.unwrap_or(1);
    let end = end_line.unwrap_or(usize::MAX);
    content
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line_number = index + 1;
            (line_number >= start && line_number <= end).then_some(line)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn merge_line_ranges(
    match_lines: &[usize],
    total_lines: usize,
    context_lines: usize,
) -> Vec<(usize, usize)> {
    let mut lines = match_lines.to_vec();
    lines.sort_unstable();
    lines.dedup();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for line in lines {
        let start = line.saturating_sub(context_lines).max(1);
        let end = line.saturating_add(context_lines).min(total_lines.max(1));
        match ranges.last_mut() {
            Some((_, previous_end)) if start <= previous_end.saturating_add(1) => {
                *previous_end = (*previous_end).max(end);
            }
            _ => ranges.push((start, end)),
        }
    }
    ranges
}

fn numbered_line_range(content: &str, start_line: usize, end_line: usize) -> String {
    content
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line_number = index + 1;
            (line_number >= start_line && line_number <= end_line)
                .then(|| format!("{line_number}: {line}"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn canonicalize_tool_path(
    tool_name: &str,
    label: &str,
    path: &Path,
) -> Result<PathBuf, RuntimeError> {
    path.canonicalize().map_err(|error| {
        RuntimeError::Provider(format!(
            "tool {tool_name} {label} {}: {error}",
            path.display()
        ))
    })
}

fn bytes_to_limited_text(bytes: &[u8], max_bytes: usize) -> (String, bool, usize) {
    let truncated = bytes.len() > max_bytes;
    let limited = if truncated {
        &bytes[..max_bytes]
    } else {
        bytes
    };
    (
        String::from_utf8_lossy(limited).to_string(),
        truncated,
        bytes.len(),
    )
}

fn resolve_config_path(config_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        config_dir.join(path)
    }
}

fn git_diff_paths(tool_name: &str, input: &Value) -> Result<Vec<String>, RuntimeError> {
    repo_tool_paths(tool_name, input)
}

fn repo_tool_paths(tool_name: &str, input: &Value) -> Result<Vec<String>, RuntimeError> {
    let mut paths = Vec::new();
    if let Some(path) = input.get("path") {
        let path = path.as_str().ok_or_else(|| {
            RuntimeError::Provider(format!("tool {tool_name} input.path must be a string"))
        })?;
        validate_git_pathspec(tool_name, path)?;
        paths.push(path.to_string());
    }
    if let Some(raw_paths) = input.get("paths") {
        let raw_paths = raw_paths.as_array().ok_or_else(|| {
            RuntimeError::Provider(format!("tool {tool_name} input.paths must be an array"))
        })?;
        for path in raw_paths {
            let path = path.as_str().ok_or_else(|| {
                RuntimeError::Provider(format!(
                    "tool {tool_name} input.paths entries must be strings"
                ))
            })?;
            validate_git_pathspec(tool_name, path)?;
            paths.push(path.to_string());
        }
    }
    Ok(paths)
}

fn parse_rg_vimgrep_line(line: &str) -> Value {
    let mut parts = line.splitn(4, ':');
    let path = parts.next().unwrap_or_default();
    let line_number = parts
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default();
    let column = parts
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default();
    let text = parts.next().unwrap_or_default();
    json!({
        "path": path,
        "line": line_number,
        "column": column,
        "text": text,
    })
}

fn validate_git_pathspec(tool_name: &str, path: &str) -> Result<(), RuntimeError> {
    let path = Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} git path filters must be relative paths inside repo_dir"
        )));
    }
    Ok(())
}

fn render_json_template_value(template: &Value, input: &Value) -> Value {
    match template {
        Value::String(text) => Value::String(render_json_template(text, input)),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| render_json_template_value(value, input))
                .collect(),
        ),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| (key.clone(), render_json_template_value(value, input)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn render_json_template(template: &str, input: &Value) -> String {
    let mut rendered = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let (before, after_start) = rest.split_at(start);
        rendered.push_str(before);
        let after_start = &after_start[2..];
        let Some(end) = after_start.find("}}") else {
            rendered.push_str("{{");
            rendered.push_str(after_start);
            return rendered;
        };
        let (path, after_end) = after_start.split_at(end);
        rendered.push_str(&template_input_value(input, path.trim()));
        rest = &after_end[2..];
    }
    rendered.push_str(rest);
    rendered
}

fn template_input_value(input: &Value, path: &str) -> String {
    let normalized = path
        .strip_prefix("input.")
        .or_else(|| path.strip_prefix('$'))
        .unwrap_or(path);
    let mut value = input;
    if !normalized.is_empty() {
        for segment in normalized.split('.') {
            let Some(next) = value.get(segment) else {
                return String::new();
            };
            value = next;
        }
    }
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

pub fn search_docs(input: &Value, documents: &[LocalDoc], max_results: usize) -> Value {
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    let mut seen = BTreeSet::new();
    let docs = documents
        .iter()
        .filter(|doc| {
            query.split_whitespace().any(|term| {
                doc.title.to_lowercase().contains(term) || doc.content.to_lowercase().contains(term)
            })
        })
        .filter(|doc| seen.insert(doc_dedupe_key(doc)))
        .take(max_results)
        .map(|doc| {
            json!({
                "id": doc.id,
                "title": doc.title,
                "content": doc.content,
                "kind": "doc_chunk",
                "uri": format!("local-doc://{}", doc.id),
            })
        })
        .collect::<Vec<_>>();
    let artifacts = docs
        .iter()
        .map(|doc| {
            json!({
                "id": doc["id"],
                "kind": "doc_chunk",
                "title": doc["title"],
                "uri": doc["uri"],
                "content": doc["content"],
                "metadata": {
                    "provider": "local_docs"
                }
            })
        })
        .collect::<Vec<_>>();

    json!({
        "query": input.get("query").cloned().unwrap_or(Value::String(String::new())),
        "documents": docs,
        "artifacts": artifacts,
    })
}

fn doc_dedupe_key(doc: &LocalDoc) -> String {
    let content = doc.content.trim().to_lowercase();
    if !content.is_empty() {
        return format!("content:{content}");
    }

    let title = doc.title.trim().to_lowercase();
    if !title.is_empty() {
        return format!("title:{title}");
    }

    format!("id:{}", doc.id.trim().to_lowercase())
}

fn call_todo_write_tool(
    name: &str,
    input: &Value,
    max_items: usize,
    max_content_chars: usize,
) -> Result<Value, RuntimeError> {
    let todos = input
        .get("todos")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            RuntimeError::Provider(format!("tool {name} input.todos must be an array"))
        })?;
    if todos.len() > max_items {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.todos must contain at most {max_items} items"
        )));
    }

    let mut seen_ids = BTreeSet::new();
    let mut normalized = Vec::new();
    let mut pending_count = 0usize;
    let mut in_progress_count = 0usize;
    let mut completed_count = 0usize;
    let mut cancelled_count = 0usize;

    for (index, todo) in todos.iter().enumerate() {
        let object = todo.as_object().ok_or_else(|| {
            RuntimeError::Provider(format!(
                "tool {name} input.todos[{index}] must be an object"
            ))
        })?;
        let id = required_object_string(name, object, &format!("todos[{index}].id"))?;
        let content = required_object_string(name, object, &format!("todos[{index}].content"))?;
        let status = required_object_string(name, object, &format!("todos[{index}].status"))?;
        let priority = required_object_string(name, object, &format!("todos[{index}].priority"))?;
        if id.trim().is_empty() {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.todos[{index}].id must not be empty"
            )));
        }
        if !seen_ids.insert(id.to_string()) {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.todos[{index}].id must be unique"
            )));
        }
        if content.trim().is_empty() {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.todos[{index}].content must not be empty"
            )));
        }
        if content.chars().count() > max_content_chars {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.todos[{index}].content must contain at most {max_content_chars} characters"
            )));
        }
        match status {
            "pending" => pending_count += 1,
            "in_progress" => in_progress_count += 1,
            "completed" => completed_count += 1,
            "cancelled" => cancelled_count += 1,
            _ => {
                return Err(RuntimeError::Provider(format!(
                    "tool {name} input.todos[{index}].status must be pending, in_progress, completed, or cancelled"
                )));
            }
        }
        if !matches!(priority, "high" | "medium" | "low") {
            return Err(RuntimeError::Provider(format!(
                "tool {name} input.todos[{index}].priority must be high, medium, or low"
            )));
        }

        normalized.push(json!({
            "id": id,
            "content": content,
            "status": status,
            "priority": priority,
        }));
    }

    if in_progress_count > 1 {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.todos must contain at most one in_progress item"
        )));
    }

    Ok(todo_list_output(
        name,
        "todo_write",
        normalized,
        TodoCounts {
            pending: pending_count,
            in_progress: in_progress_count,
            completed: completed_count,
            cancelled: cancelled_count,
        },
    ))
}

fn call_todo_read_tool(name: &str, current_todos: &[Value]) -> Value {
    let counts = todo_counts(current_todos);
    todo_list_output(name, "todo_read", current_todos.to_vec(), counts)
}

fn call_context_measure_tool(
    name: &str,
    input: &Value,
    max_context_chars: usize,
    threshold_percent: u64,
) -> Result<Value, RuntimeError> {
    if threshold_percent == 0 || threshold_percent > 100 {
        return Err(RuntimeError::Provider(format!(
            "tool {name} threshold_percent must be between 1 and 100"
        )));
    }
    let effective_max_context_chars =
        optional_bounded_usize_input(name, input, "max_context_chars", max_context_chars)?
            .unwrap_or(max_context_chars);
    let effective_threshold_percent =
        optional_threshold_percent_input(name, input, "threshold_percent")?
            .unwrap_or(threshold_percent);
    let payload = input.get("payload").unwrap_or(input);
    let chars = json_char_count(payload);
    let threshold_chars =
        effective_max_context_chars.saturating_mul(effective_threshold_percent as usize) / 100;
    let should_compact = chars >= threshold_chars;
    let fields = payload
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(field, value)| {
                    json!({
                        "field": field,
                        "chars": json_char_count(value)
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let content = format!(
        "chars: {chars}\nmax_context_chars: {effective_max_context_chars}\nthreshold_percent: {effective_threshold_percent}\nthreshold_chars: {threshold_chars}\nshould_compact: {should_compact}"
    );
    Ok(json!({
        "chars": chars,
        "max_context_chars": effective_max_context_chars,
        "threshold_percent": effective_threshold_percent,
        "threshold_chars": threshold_chars,
        "usage_ratio": chars as f64 / effective_max_context_chars.max(1) as f64,
        "should_compact": should_compact,
        "fields": fields,
        "artifacts": [{
            "id": format!("context-measure:{chars}:{threshold_chars}"),
            "kind": "context_measure",
            "title": "context measure",
            "uri": "air://context/measure",
            "content": content,
            "metadata": {
                "provider": "context_measure",
                "chars": chars,
                "max_context_chars": effective_max_context_chars,
                "threshold_percent": effective_threshold_percent,
                "threshold_chars": threshold_chars,
                "should_compact": should_compact
            }
        }]
    }))
}

fn json_char_count(value: &Value) -> usize {
    serde_json::to_string(value)
        .unwrap_or_default()
        .chars()
        .count()
}

#[derive(Debug, Clone, Copy)]
struct TodoCounts {
    pending: usize,
    in_progress: usize,
    completed: usize,
    cancelled: usize,
}

fn todo_counts(todos: &[Value]) -> TodoCounts {
    let mut counts = TodoCounts {
        pending: 0,
        in_progress: 0,
        completed: 0,
        cancelled: 0,
    };
    for todo in todos {
        match todo
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "pending" => counts.pending += 1,
            "in_progress" => counts.in_progress += 1,
            "completed" => counts.completed += 1,
            "cancelled" => counts.cancelled += 1,
            _ => {}
        }
    }
    counts
}

fn todo_list_output(name: &str, provider: &str, todos: Vec<Value>, counts: TodoCounts) -> Value {
    let open_count = counts.pending + counts.in_progress;
    let content = serde_json::to_string_pretty(&todos).unwrap_or_else(|_| "[]".to_string());
    json!({
        "todos": todos,
        "total": todos.len(),
        "open_count": open_count,
        "pending_count": counts.pending,
        "in_progress_count": counts.in_progress,
        "completed_count": counts.completed,
        "cancelled_count": counts.cancelled,
        "artifacts": [{
            "id": "todo:list",
            "kind": "todo_list",
            "title": format!("{open_count} open todos"),
            "uri": "air://todos/current",
            "content": content,
            "metadata": {
                "provider": provider,
                "tool": name,
                "total": todos.len(),
                "open_count": open_count,
                "pending_count": counts.pending,
                "in_progress_count": counts.in_progress,
                "completed_count": counts.completed,
                "cancelled_count": counts.cancelled
            }
        }],
    })
}

fn required_object_string<'a>(
    tool_name: &str,
    object: &'a Map<String, Value>,
    field: &str,
) -> Result<&'a str, RuntimeError> {
    object
        .get(field.rsplit('.').next().unwrap_or(field))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            RuntimeError::Provider(format!("tool {tool_name} input.{field} must be a string"))
        })
}

struct HelpdeskDoc {
    id: &'static str,
    title: &'static str,
    content: &'static str,
}

fn helpdesk_docs() -> Vec<HelpdeskDoc> {
    vec![
        HelpdeskDoc {
            id: "kb-password-reset",
            title: "Reset your password",
            content: "Users can reset a password from the sign-in page by selecting Forgot password, entering the account email, and following the reset link. Reset links expire after 30 minutes.",
        },
        HelpdeskDoc {
            id: "kb-lost-email-access",
            title: "Account recovery when email is unavailable",
            content: "If a user no longer has access to the account email, support must verify identity with the last invoice id and the last four digits of the payment method before changing the email address.",
        },
        HelpdeskDoc {
            id: "kb-billing-upgrade",
            title: "Billing after subscription upgrade",
            content: "After an upgrade, a prorated charge may appear immediately. Duplicate charges should be escalated to billing support with invoice ids.",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_config(dir: &Path, content: &str) -> PathBuf {
        let path = dir.join("tools.json");
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn todo_write_returns_a_structured_artifact() {
        let dir = temp_dir("air-tools-todo-write");
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "todo.write": {
                  "kind": "todo_write",
                  "capability": "task.progress",
                  "max_items": 4,
                  "max_content_chars": 80
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool(
                "todo.write",
                &json!({
                    "todos": [
                        {
                            "id": "inspect",
                            "content": "Inspect repository context",
                            "status": "completed",
                            "priority": "high"
                        },
                        {
                            "id": "fix",
                            "content": "Apply the bounded fix",
                            "status": "in_progress",
                            "priority": "high"
                        },
                        {
                            "id": "verify",
                            "content": "Run the allowlisted verification",
                            "status": "pending",
                            "priority": "medium"
                        }
                    ]
                }),
            )
            .unwrap();

        assert_eq!(output["total"], json!(3));
        assert_eq!(output["open_count"], json!(2));
        assert_eq!(output["in_progress_count"], json!(1));
        assert_eq!(output["artifacts"][0]["kind"], json!("todo_list"));
        assert_eq!(tools.tool_capability("todo.write"), Some("task.progress"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn todo_read_returns_current_todo_list_after_write() {
        let dir = temp_dir("air-tools-todo-read");
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "todo.write": {
                  "kind": "todo_write",
                  "capability": "task.progress"
                },
                "todo.read": {
                  "kind": "todo_read",
                  "capability": "task.progress"
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let empty = tools.call_tool("todo.read", &json!({})).unwrap();
        assert_eq!(empty["total"], json!(0));
        assert_eq!(empty["open_count"], json!(0));
        assert_eq!(
            empty["artifacts"][0]["metadata"]["provider"],
            json!("todo_read")
        );

        tools
            .call_tool(
                "todo.write",
                &json!({
                    "todos": [
                        {
                            "id": "inspect",
                            "content": "Inspect repository context",
                            "status": "completed",
                            "priority": "high"
                        },
                        {
                            "id": "verify",
                            "content": "Run verification",
                            "status": "pending",
                            "priority": "medium"
                        }
                    ]
                }),
            )
            .unwrap();

        let output = tools.call_tool("todo.read", &json!({})).unwrap();
        assert_eq!(output["total"], json!(2));
        assert_eq!(output["open_count"], json!(1));
        assert_eq!(output["completed_count"], json!(1));
        assert_eq!(output["todos"][1]["id"], json!("verify"));
        assert_eq!(tools.tool_capability("todo.read"), Some("task.progress"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn context_measure_triggers_when_payload_crosses_threshold() {
        let dir = temp_dir("air-tools-context-measure");
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "context.measure": {
                  "kind": "context_measure",
                  "capability": "context.manage",
                  "max_context_chars": 100,
                  "threshold_percent": 80
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool(
                "context.measure",
                &json!({"payload": {"large": "x".repeat(100), "small": "ok"}}),
            )
            .unwrap();

        assert_eq!(output["should_compact"], json!(true));
        assert_eq!(output["threshold_chars"], json!(80));
        assert_eq!(output["artifacts"][0]["kind"], json!("context_measure"));
        assert_eq!(
            tools.tool_capability("context.measure"),
            Some("context.manage")
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn context_measure_respects_input_threshold_override() {
        let dir = temp_dir("air-tools-context-measure-override");
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "context.measure": {
                  "kind": "context_measure",
                  "max_context_chars": 1000,
                  "threshold_percent": 80
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool(
                "context.measure",
                &json!({
                    "payload": {"small": "ok"},
                    "max_context_chars": 100,
                    "threshold_percent": 1
                }),
            )
            .unwrap();

        assert_eq!(output["should_compact"], json!(true));
        assert_eq!(output["threshold_percent"], json!(1));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn todo_write_rejects_multiple_in_progress_items() {
        let dir = temp_dir("air-tools-todo-write-invalid");
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "todo.write": {
                  "kind": "todo_write",
                  "capability": "task.progress"
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let error = tools
            .call_tool(
                "todo.write",
                &json!({
                    "todos": [
                        {
                            "id": "one",
                            "content": "First task",
                            "status": "in_progress",
                            "priority": "high"
                        },
                        {
                            "id": "two",
                            "content": "Second task",
                            "status": "in_progress",
                            "priority": "medium"
                        }
                    ]
                }),
            )
            .unwrap_err();

        assert!(error.to_string().contains("at most one in_progress"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_read_reads_inside_configured_base_dir() {
        let dir = temp_dir("air-tools-file-read");
        let docs = dir.join("docs");
        fs::create_dir_all(&docs).unwrap();
        fs::write(docs.join("note.txt"), "hello from AIR tools").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "docs",
                  "max_bytes": 8
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool("file.read", &json!({"path": "note.txt"}))
            .unwrap();

        assert_eq!(output["content"], json!("hello fr"));
        assert_eq!(output["truncated"], json!(true));
        assert_eq!(output["artifacts"][0]["kind"], json!("file_span"));
        assert!(output["artifacts"][0]["id"]
            .as_str()
            .unwrap()
            .starts_with("file:"));
        assert_eq!(tools.tool_capability("file.read"), Some("file.read"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_read_many_reads_multiple_files_with_artifacts() {
        let dir = temp_dir("air-tools-file-read-many");
        fs::write(dir.join("one.txt"), "alpha\nbeta\n").unwrap();
        fs::write(dir.join("two.txt"), "first\nneedle\nlast\n").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read_many": {
                  "kind": "file_read_many",
                  "capability": "file.read",
                  "base_dir": ".",
                  "max_files": 3,
                  "max_bytes": 1024
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool(
                "file.read_many",
                &json!({
                    "files": [
                        "one.txt",
                        {
                            "path": "two.txt",
                            "contains": "needle",
                            "context_lines": 1,
                            "line_numbers": true
                        }
                    ]
                }),
            )
            .unwrap();

        assert_eq!(output["file_count"], json!(2));
        assert_eq!(output["files"][0]["content"], json!("alpha\nbeta\n"));
        assert_eq!(output["files"][1]["match_line"], json!(2));
        assert_eq!(
            output["files"][1]["numbered_content"],
            json!("00001| first\n00002| needle\n00003| last")
        );
        assert_eq!(output["artifacts"].as_array().unwrap().len(), 2);
        assert_eq!(tools.tool_capability("file.read_many"), Some("file.read"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_read_many_rejects_more_than_configured_max_files() {
        let dir = temp_dir("air-tools-file-read-many-max");
        fs::write(dir.join("one.txt"), "one").unwrap();
        fs::write(dir.join("two.txt"), "two").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read_many": {
                  "kind": "file_read_many",
                  "capability": "file.read",
                  "base_dir": ".",
                  "max_files": 1
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let error = tools
            .call_tool("file.read_many", &json!({"files": ["one.txt", "two.txt"]}))
            .unwrap_err();

        assert!(error.to_string().contains("at most 1 files"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_read_rejects_path_outside_base_dir() {
        let dir = temp_dir("air-tools-file-read-boundary");
        let docs = dir.join("docs");
        fs::create_dir_all(&docs).unwrap();
        fs::write(dir.join("secret.txt"), "secret").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "docs"
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let error = tools
            .call_tool("file.read", &json!({"path": "../secret.txt"}))
            .unwrap_err();

        assert!(error.to_string().contains("outside configured base_dir"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_read_rejects_binary_content() {
        let dir = temp_dir("air-tools-file-read-binary");
        fs::write(dir.join("blob.bin"), [0, 159, 146, 150, 0, 1]).unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let error = tools
            .call_tool("file.read", &json!({"path": "blob.bin"}))
            .unwrap_err();

        assert!(error.to_string().contains("appears to be binary"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_read_rejects_non_utf8_text() {
        let dir = temp_dir("air-tools-file-read-non-utf8");
        fs::write(dir.join("latin1.txt"), [b'h', b'i', 0xff]).unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let error = tools
            .call_tool("file.read", &json!({"path": "latin1.txt"}))
            .unwrap_err();

        assert!(error.to_string().contains("not valid UTF-8"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_read_can_return_numbered_content() {
        let dir = temp_dir("air-tools-file-read-numbered");
        fs::write(dir.join("note.txt"), "one\ntwo\nthree\n").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool(
                "file.read",
                &json!({"path": "note.txt", "start_line": 2, "line_numbers": true}),
            )
            .unwrap();

        assert_eq!(output["content"], json!("two\nthree"));
        assert_eq!(output["line_numbers"], json!(true));
        assert_eq!(
            output["numbered_content"],
            json!("00002| two\n00003| three")
        );
        assert_eq!(
            output["artifacts"][0]["metadata"]["line_numbers"],
            json!(true)
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_write_writes_inside_configured_base_dir() {
        let dir = temp_dir("air-tools-file-write");
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.write": {
                  "kind": "file_write",
                  "capability": "file.write",
                  "base_dir": ".",
                  "create_dirs": true,
                  "allow_overwrite": true,
                  "max_bytes": 1024
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool(
                "file.write",
                &json!({"path": "site/index.html", "content": "<h1>AIR</h1>"}),
            )
            .unwrap();

        assert_eq!(
            fs::read_to_string(dir.join("site/index.html")).unwrap(),
            "<h1>AIR</h1>"
        );
        assert_eq!(output["created"], json!(true));
        assert_eq!(output["artifacts"][0]["kind"], json!("file_write"));
        assert_eq!(tools.tool_capability("file.write"), Some("file.write"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_write_rejects_parent_path_escape() {
        let dir = temp_dir("air-tools-file-write-boundary");
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.write": {
                  "kind": "file_write",
                  "capability": "file.write",
                  "base_dir": ".",
                  "create_dirs": true
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let error = tools
            .call_tool(
                "file.write",
                &json!({"path": "../escape.html", "content": "x"}),
            )
            .unwrap_err();

        assert!(error.to_string().contains("relative paths inside repo_dir"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_write_requires_read_before_overwrite_when_configured() {
        let dir = temp_dir("air-tools-file-write-read-first");
        fs::write(dir.join("note.txt"), "before").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.write": {
                  "kind": "file_write",
                  "capability": "file.write",
                  "base_dir": ".",
                  "allow_overwrite": true,
                  "require_read": true
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let error = tools
            .call_tool(
                "file.write",
                &json!({"path": "note.txt", "content": "after"}),
            )
            .unwrap_err();
        assert!(error.to_string().contains("must be read before overwrite"));

        tools
            .call_tool("file.read", &json!({"path": "note.txt"}))
            .unwrap();
        let output = tools
            .call_tool(
                "file.write",
                &json!({"path": "note.txt", "content": "after"}),
            )
            .unwrap();

        assert_eq!(fs::read_to_string(dir.join("note.txt")).unwrap(), "after");
        assert_eq!(output["overwritten"], json!(true));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_write_rejects_stale_read_before_overwrite() {
        let dir = temp_dir("air-tools-file-write-stale-read");
        fs::write(dir.join("note.txt"), "before").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.write": {
                  "kind": "file_write",
                  "capability": "file.write",
                  "base_dir": ".",
                  "allow_overwrite": true,
                  "require_read": true
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();
        tools
            .call_tool("file.read", &json!({"path": "note.txt"}))
            .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        fs::write(dir.join("note.txt"), "outside change").unwrap();

        let error = tools
            .call_tool(
                "file.write",
                &json!({"path": "note.txt", "content": "agent change"}),
            )
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("modified after it was last read"));
        assert_eq!(
            fs::read_to_string(dir.join("note.txt")).unwrap(),
            "outside change"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_edit_replaces_unique_string_after_read() {
        let dir = temp_dir("air-tools-file-edit");
        fs::write(dir.join("note.txt"), "hello AIR\n").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.edit": {
                  "kind": "file_edit",
                  "capability": "file.write",
                  "base_dir": ".",
                  "max_bytes": 1024
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let error = tools
            .call_tool(
                "file.edit",
                &json!({"path": "note.txt", "old_string": "AIR", "new_string": "agent IR"}),
            )
            .unwrap_err();
        assert!(error.to_string().contains("must be read before edit"));

        tools
            .call_tool("file.read", &json!({"path": "note.txt"}))
            .unwrap();
        let output = tools
            .call_tool(
                "file.edit",
                &json!({"path": "note.txt", "old_string": "AIR", "new_string": "agent IR"}),
            )
            .unwrap();

        assert_eq!(
            fs::read_to_string(dir.join("note.txt")).unwrap(),
            "hello agent IR\n"
        );
        assert_eq!(output["replacements"], json!(1));
        assert_eq!(output["diff_truncated"], json!(false));
        assert!(output["diff"].as_str().unwrap().contains("-AIR"));
        assert!(output["diff"].as_str().unwrap().contains("+agent IR"));
        assert_eq!(output["artifacts"][0]["content"], output["diff"]);
        assert_eq!(output["artifacts"][0]["kind"], json!("file_edit"));
        assert_eq!(tools.tool_capability("file.edit"), Some("file.write"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_edit_applies_multiple_edits_after_read() {
        let dir = temp_dir("air-tools-file-edit-multiple-ops");
        fs::write(
            dir.join("note.txt"),
            "title: draft\nstatus: todo\nowner: unknown\n",
        )
        .unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.edit": {
                  "kind": "file_edit",
                  "capability": "file.write",
                  "base_dir": ".",
                  "max_bytes": 1024
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();
        tools
            .call_tool("file.read", &json!({"path": "note.txt"}))
            .unwrap();

        let output = tools
            .call_tool(
                "file.edit",
                &json!({
                    "path": "note.txt",
                    "edits": [
                        {
                            "old_string": "title: draft",
                            "new_string": "title: ready"
                        },
                        {
                            "old_string": "owner: unknown",
                            "new_string": "owner: agent"
                        }
                    ]
                }),
            )
            .unwrap();

        assert_eq!(
            fs::read_to_string(dir.join("note.txt")).unwrap(),
            "title: ready\nstatus: todo\nowner: agent\n"
        );
        assert_eq!(output["edit_count"], json!(2));
        assert_eq!(output["replacements"], json!(2));
        assert_eq!(output["match_strategies"], json!(["exact", "exact"]));
        assert!(output["diff"].as_str().unwrap().contains("-title: draft"));
        assert!(output["diff"].as_str().unwrap().contains("+owner: agent"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_edit_multi_edit_is_atomic_when_later_edit_fails() {
        let dir = temp_dir("air-tools-file-edit-multiple-atomic");
        fs::write(dir.join("note.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.edit": {
                  "kind": "file_edit",
                  "capability": "file.write",
                  "base_dir": "."
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();
        tools
            .call_tool("file.read", &json!({"path": "note.txt"}))
            .unwrap();

        let error = tools
            .call_tool(
                "file.edit",
                &json!({
                    "path": "note.txt",
                    "edits": [
                        {
                            "old_string": "alpha",
                            "new_string": "ALPHA"
                        },
                        {
                            "old_string": "missing",
                            "new_string": "MISSING"
                        }
                    ]
                }),
            )
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("input.edits[1].old_string was not found"));
        assert_eq!(
            fs::read_to_string(dir.join("note.txt")).unwrap(),
            "alpha\nbeta\ngamma\n"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_edit_rejects_stale_read() {
        let dir = temp_dir("air-tools-file-edit-stale-read");
        fs::write(dir.join("note.txt"), "hello AIR\n").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.edit": {
                  "kind": "file_edit",
                  "capability": "file.write",
                  "base_dir": "."
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();
        tools
            .call_tool("file.read", &json!({"path": "note.txt"}))
            .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        fs::write(dir.join("note.txt"), "hello outside\n").unwrap();

        let error = tools
            .call_tool(
                "file.edit",
                &json!({"path": "note.txt", "old_string": "outside", "new_string": "agent"}),
            )
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("modified after it was last read"));
        assert_eq!(
            fs::read_to_string(dir.join("note.txt")).unwrap(),
            "hello outside\n"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_edit_supports_explicit_line_trimmed_match_strategy() {
        let dir = temp_dir("air-tools-file-edit-line-trimmed");
        fs::write(
            dir.join("note.txt"),
            "function demo() {\n    return \"before\";\n}\n",
        )
        .unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.edit": {
                  "kind": "file_edit",
                  "capability": "file.write",
                  "base_dir": "."
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();
        tools
            .call_tool("file.read", &json!({"path": "note.txt"}))
            .unwrap();

        let exact_error = tools
            .call_tool(
                "file.edit",
                &json!({
                    "path": "note.txt",
                    "old_string": "function demo() {\nreturn \"before\";\n}",
                    "new_string": "function demo() {\n    return \"after\";\n}"
                }),
            )
            .unwrap_err();
        assert!(exact_error.to_string().contains("match_strategy=exact"));

        let output = tools
            .call_tool(
                "file.edit",
                &json!({
                    "path": "note.txt",
                    "old_string": "function demo() {\nreturn \"before\";\n}",
                    "new_string": "function demo() {\n    return \"after\";\n}",
                    "match_strategy": "line_trimmed"
                }),
            )
            .unwrap();

        assert_eq!(output["match_strategy"], json!("line_trimmed"));
        assert_eq!(
            fs::read_to_string(dir.join("note.txt")).unwrap(),
            "function demo() {\n    return \"after\";\n}\n"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_edit_rejects_unknown_match_strategy() {
        let dir = temp_dir("air-tools-file-edit-match-strategy");
        fs::write(dir.join("note.txt"), "hello AIR\n").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.edit": {
                  "kind": "file_edit",
                  "capability": "file.write",
                  "base_dir": "."
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();
        tools
            .call_tool("file.read", &json!({"path": "note.txt"}))
            .unwrap();

        let error = tools
            .call_tool(
                "file.edit",
                &json!({
                    "path": "note.txt",
                    "old_string": "AIR",
                    "new_string": "Agent",
                    "match_strategy": "semantic_guess"
                }),
            )
            .unwrap_err();

        assert!(error.to_string().contains("input.match_strategy"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_edit_rejects_noop_replacement() {
        let dir = temp_dir("air-tools-file-edit-noop");
        fs::write(dir.join("note.txt"), "hello AIR\n").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.edit": {
                  "kind": "file_edit",
                  "capability": "file.write",
                  "base_dir": "."
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();
        tools
            .call_tool("file.read", &json!({"path": "note.txt"}))
            .unwrap();

        let error = tools
            .call_tool(
                "file.edit",
                &json!({"path": "note.txt", "old_string": "AIR", "new_string": "AIR"}),
            )
            .unwrap_err();

        assert!(error.to_string().contains("must be different"));
        assert_eq!(
            fs::read_to_string(dir.join("note.txt")).unwrap(),
            "hello AIR\n"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_edit_rejects_multiple_matches_without_replace_all() {
        let dir = temp_dir("air-tools-file-edit-multiple");
        fs::write(dir.join("note.txt"), "AIR AIR\n").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.edit": {
                  "kind": "file_edit",
                  "capability": "file.write",
                  "base_dir": ".",
                  "allow_replace_all": true
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();
        tools
            .call_tool("file.read", &json!({"path": "note.txt"}))
            .unwrap();

        let error = tools
            .call_tool(
                "file.edit",
                &json!({"path": "note.txt", "old_string": "AIR", "new_string": "Agent"}),
            )
            .unwrap_err();
        assert!(error.to_string().contains("matched 2 times"));

        let output = tools
            .call_tool(
                "file.edit",
                &json!({
                    "path": "note.txt",
                    "old_string": "AIR",
                    "new_string": "Agent",
                    "replace_all": true
                }),
            )
            .unwrap();
        assert_eq!(output["replacements"], json!(2));
        assert_eq!(
            fs::read_to_string(dir.join("note.txt")).unwrap(),
            "Agent Agent\n"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_patch_applies_unified_diff_after_read() {
        let dir = temp_dir("air-tools-file-patch");
        fs::write(dir.join("note.txt"), "before\n").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.patch": {
                  "kind": "file_patch",
                  "capability": "file.write",
                  "repo_dir": ".",
                  "require_read": true,
                  "max_files": 3
                }
              }
            }"#,
        );
        let patch = "diff --git a/note.txt b/note.txt\n--- a/note.txt\n+++ b/note.txt\n@@ -1 +1 @@\n-before\n+after\n";
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let error = tools
            .call_tool("file.patch", &json!({"patch": patch}))
            .unwrap_err();
        assert!(error.to_string().contains("must be read before patch"));

        tools
            .call_tool("file.read", &json!({"path": "note.txt"}))
            .unwrap();
        let output = tools
            .call_tool("file.patch", &json!({"patch": patch}))
            .unwrap();

        assert_eq!(fs::read_to_string(dir.join("note.txt")).unwrap(), "after\n");
        assert_eq!(output["file_count"], json!(1));
        assert_eq!(output["artifacts"][0]["kind"], json!("file_patch"));
        assert_eq!(tools.tool_capability("file.patch"), Some("file.write"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_patch_rejects_stale_read() {
        let dir = temp_dir("air-tools-file-patch-stale-read");
        fs::write(dir.join("note.txt"), "before\n").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.patch": {
                  "kind": "file_patch",
                  "capability": "file.write",
                  "repo_dir": ".",
                  "require_read": true
                }
              }
            }"#,
        );
        let patch = "diff --git a/note.txt b/note.txt\n--- a/note.txt\n+++ b/note.txt\n@@ -1 +1 @@\n-outside\n+after\n";
        let mut tools = ConfigTools::from_file(config_path).unwrap();
        tools
            .call_tool("file.read", &json!({"path": "note.txt"}))
            .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        fs::write(dir.join("note.txt"), "outside\n").unwrap();

        let error = tools
            .call_tool("file.patch", &json!({"patch": patch}))
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("modified after it was last read"));
        assert_eq!(
            fs::read_to_string(dir.join("note.txt")).unwrap(),
            "outside\n"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_patch_can_create_new_file_when_allowed() {
        let dir = temp_dir("air-tools-file-patch-new");
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.patch": {
                  "kind": "file_patch",
                  "capability": "file.write",
                  "repo_dir": ".",
                  "allow_new_files": true
                }
              }
            }"#,
        );
        let patch = "diff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+hello\n";
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool("file.patch", &json!({"patch": patch}))
            .unwrap();

        assert_eq!(fs::read_to_string(dir.join("new.txt")).unwrap(), "hello\n");
        assert_eq!(output["files"][0]["kind"], json!("new"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_patch_dry_run_returns_failed_check_without_applying() {
        let dir = temp_dir("air-tools-file-patch-dry-run");
        fs::write(dir.join("note.txt"), "before\n").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                },
                "file.patch": {
                  "kind": "file_patch",
                  "capability": "file.write",
                  "repo_dir": ".",
                  "require_read": true
                }
              }
            }"#,
        );
        let patch = "diff --git a/note.txt b/note.txt\n--- a/note.txt\n+++ b/note.txt\n@@ -1 +1 @@\n-not-present\n+after\n";
        let mut tools = ConfigTools::from_file(config_path).unwrap();
        tools
            .call_tool("file.read", &json!({"path": "note.txt"}))
            .unwrap();

        let output = tools
            .call_tool("file.patch", &json!({"patch": patch, "dry_run": true}))
            .unwrap();

        assert_eq!(output["success"], json!(false));
        assert_eq!(output["checked"], json!(true));
        assert_eq!(output["applied"], json!(false));
        assert_eq!(output["diagnostics"][0]["source"], json!("file_patch"));
        assert_eq!(
            fs::read_to_string(dir.join("note.txt")).unwrap(),
            "before\n"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_patch_rejects_parent_path_escape() {
        let dir = temp_dir("air-tools-file-patch-boundary");
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.patch": {
                  "kind": "file_patch",
                  "capability": "file.write",
                  "repo_dir": ".",
                  "require_read": false
                }
              }
            }"#,
        );
        let patch = "diff --git a/../escape.txt b/../escape.txt\n--- a/../escape.txt\n+++ b/../escape.txt\n@@ -1 +1 @@\n-before\n+after\n";
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let error = tools
            .call_tool("file.patch", &json!({"patch": patch}))
            .unwrap_err();

        assert!(error.to_string().contains("relative paths inside repo_dir"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_read_can_return_line_ranges() {
        let dir = temp_dir("air-tools-file-read-range");
        fs::write(dir.join("note.txt"), "one\ntwo\nthree\nfour\n").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool(
                "file.read",
                &json!({"path": "note.txt", "start_line": 2, "end_line": 3}),
            )
            .unwrap();

        assert_eq!(output["content"], json!("two\nthree"));
        assert_eq!(output["start_line"], json!(2));
        assert_eq!(output["end_line"], json!(3));
        assert_eq!(output["total_lines"], json!(4));
        assert_eq!(output["artifacts"][0]["metadata"]["start_line"], json!(2));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_read_can_return_context_around_contains_match() {
        let dir = temp_dir("air-tools-file-read-contains");
        fs::write(
            dir.join("note.txt"),
            "alpha\nbefore\ntarget symbol\nafter\nomega\n",
        )
        .unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool(
                "file.read",
                &json!({
                    "path": "note.txt",
                    "contains": "target",
                    "context_lines": 1,
                    "line_numbers": true
                }),
            )
            .unwrap();

        assert_eq!(output["content"], json!("before\ntarget symbol\nafter"));
        assert_eq!(output["start_line"], json!(2));
        assert_eq!(output["end_line"], json!(4));
        assert_eq!(output["match_line"], json!(3));
        assert_eq!(
            output["numbered_content"],
            json!("00002| before\n00003| target symbol\n00004| after")
        );
        assert_eq!(
            output["artifacts"][0]["metadata"]["contains"],
            json!("target")
        );
        assert_eq!(output["occurrence"], json!(1));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_read_can_return_context_around_later_contains_occurrence() {
        let dir = temp_dir("air-tools-file-read-contains-occurrence");
        fs::write(
            dir.join("note.txt"),
            "target first\nmiddle\nbefore\ntarget second\nafter\n",
        )
        .unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool(
                "file.read",
                &json!({
                    "path": "note.txt",
                    "contains": "target",
                    "occurrence": 2,
                    "context_lines": 1
                }),
            )
            .unwrap();

        assert_eq!(output["content"], json!("before\ntarget second\nafter"));
        assert_eq!(output["match_line"], json!(4));
        assert_eq!(output["occurrence"], json!(2));
        assert_eq!(output["artifacts"][0]["metadata"]["occurrence"], json!(2));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_read_rejects_missing_contains_without_returning_wrong_context() {
        let dir = temp_dir("air-tools-file-read-contains-missing");
        fs::write(dir.join("note.txt"), "alpha\nbeta\n").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let error = tools
            .call_tool(
                "file.read",
                &json!({"path": "note.txt", "contains": "gamma"}),
            )
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("input.contains occurrence=1 was not found"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_read_rejects_missing_contains_occurrence() {
        let dir = temp_dir("air-tools-file-read-contains-occurrence-missing");
        fs::write(dir.join("note.txt"), "target once\n").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "file.read": {
                  "kind": "file_read",
                  "capability": "file.read",
                  "base_dir": "."
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let error = tools
            .call_tool(
                "file.read",
                &json!({"path": "note.txt", "contains": "target", "occurrence": 2}),
            )
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("input.contains occurrence=2 was not found"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn git_diff_returns_diff_for_configured_repo() {
        let dir = temp_dir("air-tools-git-diff");
        Command::new("git")
            .arg("-C")
            .arg(&dir)
            .arg("init")
            .output()
            .unwrap();
        fs::write(dir.join("note.txt"), "before\n").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["add", "note.txt"])
            .output()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["commit", "-m", "init"])
            .env("GIT_AUTHOR_NAME", "AIR")
            .env("GIT_AUTHOR_EMAIL", "air@example.com")
            .env("GIT_COMMITTER_NAME", "AIR")
            .env("GIT_COMMITTER_EMAIL", "air@example.com")
            .output()
            .unwrap();
        fs::write(dir.join("note.txt"), "after\n").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "git.diff": {
                  "kind": "git_diff",
                  "capability": "code.read",
                  "repo_dir": "."
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool("git.diff", &json!({"path": "note.txt"}))
            .unwrap();

        assert!(output["diff"].as_str().unwrap().contains("-before"));
        assert!(output["diff"].as_str().unwrap().contains("+after"));
        assert_eq!(output["artifacts"][0]["kind"], json!("git_diff"));
        assert!(output["artifacts"][0]["id"]
            .as_str()
            .unwrap()
            .contains("note.txt"));
        assert_eq!(tools.tool_capability("git.diff"), Some("code.read"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn git_diff_rejects_parent_path_filters() {
        let dir = temp_dir("air-tools-git-diff-path");
        Command::new("git")
            .arg("-C")
            .arg(&dir)
            .arg("init")
            .output()
            .unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "git.diff": {
                  "kind": "git_diff",
                  "capability": "code.read",
                  "repo_dir": "."
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let error = tools
            .call_tool("git.diff", &json!({"path": "../outside"}))
            .unwrap_err();

        assert!(error.to_string().contains("relative paths inside repo_dir"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn git_status_returns_structured_workspace_entries() {
        let dir = temp_dir("air-tools-git-status");
        Command::new("git")
            .arg("-C")
            .arg(&dir)
            .arg("init")
            .output()
            .unwrap();
        fs::write(dir.join("tracked.txt"), "before\n").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["add", "tracked.txt"])
            .output()
            .unwrap();
        fs::write(dir.join("tracked.txt"), "after\n").unwrap();
        fs::write(dir.join("new.txt"), "new\n").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "git.status": {
                  "kind": "git_status",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_files": 10
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools.call_tool("git.status", &json!({})).unwrap();

        assert_eq!(output["clean"], json!(false));
        assert!(output["file_count"].as_u64().unwrap() >= 2);
        assert!(output["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == json!("tracked.txt")));
        assert!(output["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |entry| entry["path"] == json!("new.txt") && entry["status"] == json!("untracked")
            ));
        assert_eq!(output["artifacts"][0]["kind"], json!("git_status"));
        assert_eq!(tools.tool_capability("git.status"), Some("code.read"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn repo_files_lists_and_filters_repo_paths() {
        let dir = temp_dir("air-tools-repo-files");
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
        fs::write(dir.join("README.md"), "alpha docs\n").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "repo.files": {
                  "kind": "repo_files",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_files": 10
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool("repo.files", &json!({"query": "lib"}))
            .unwrap();

        assert_eq!(output["files"], json!(["src/lib.rs"]));
        assert_eq!(output["artifacts"][0]["kind"], json!("repo_listing"));
        assert_eq!(tools.tool_capability("repo.files"), Some("code.read"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn repo_search_returns_structured_matches() {
        let dir = temp_dir("air-tools-repo-search");
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("src/lib.rs"),
            "pub fn alpha() {}\npub fn beta() { alpha(); }\n",
        )
        .unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "repo.search": {
                  "kind": "repo_search",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_matches": 5
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool("repo.search", &json!({"query": "alpha", "path": "src"}))
            .unwrap();

        assert_eq!(output["matches"][0]["path"], json!("src/lib.rs"));
        assert_eq!(output["matches"][0]["line"], json!(1));
        assert!(output["artifacts"][0]["content"]
            .as_str()
            .unwrap()
            .contains("alpha"));
        assert_eq!(tools.tool_capability("repo.search"), Some("code.read"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn repo_search_supports_explicit_regex_mode() {
        let dir = temp_dir("air-tools-repo-search-regex");
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("src/lib.rs"),
            "pub fn alpha() {}\nfn beta_value() {}\nfn gammaValue() {}\n",
        )
        .unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "repo.search": {
                  "kind": "repo_search",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_matches": 5
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool(
                "repo.search",
                &json!({"query": "fn [a-z]+_value", "mode": "regex", "path": "src"}),
            )
            .unwrap();

        assert_eq!(output["mode"], json!("regex"));
        assert_eq!(output["matches"].as_array().unwrap().len(), 1);
        assert!(output["matches"][0]["text"]
            .as_str()
            .unwrap()
            .contains("beta_value"));
        assert_eq!(output["artifacts"][0]["metadata"]["mode"], json!("regex"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn repo_search_supports_bounded_per_call_max_matches() {
        let dir = temp_dir("air-tools-repo-search-max-matches");
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("src/lib.rs"),
            "alpha one\nalpha two\nalpha three\nalpha four\n",
        )
        .unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "repo.search": {
                  "kind": "repo_search",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_matches": 3
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool(
                "repo.search",
                &json!({"query": "alpha", "path": "src", "max_matches": 2}),
            )
            .unwrap();

        assert_eq!(output["matches"].as_array().unwrap().len(), 2);
        assert_eq!(output["truncated"], json!(true));
        assert_eq!(output["artifacts"][0]["metadata"]["max_matches"], json!(2));

        let capped = tools
            .call_tool(
                "repo.search",
                &json!({"query": "alpha", "path": "src", "max_matches": 99}),
            )
            .unwrap();
        assert_eq!(capped["matches"].as_array().unwrap().len(), 3);
        assert_eq!(capped["artifacts"][0]["metadata"]["max_matches"], json!(3));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn repo_search_rejects_unknown_mode() {
        let dir = temp_dir("air-tools-repo-search-mode");
        fs::write(dir.join("lib.rs"), "fn alpha() {}\n").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "repo.search": {
                  "kind": "repo_search",
                  "capability": "code.read",
                  "repo_dir": "."
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let error = tools
            .call_tool("repo.search", &json!({"query": "alpha", "mode": "glob"}))
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("input.mode must be fixed or regex"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn repo_context_returns_nearby_code_snippets() {
        let dir = temp_dir("air-tools-repo-context");
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("src/lib.rs"),
            "line 1\nline 2\nfn alpha() {}\nline 4\nline 5\nline 6\nfn beta() { alpha(); }\nline 8\n",
        )
        .unwrap();
        fs::write(dir.join("src/other.rs"), "fn alpha_other() {}\n").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "repo.context": {
                  "kind": "repo_context",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_matches": 10,
                  "max_files": 1,
                  "context_lines": 1,
                  "max_bytes": 4096
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool(
                "repo.context",
                &json!({"query": "alpha", "path": "src/lib.rs"}),
            )
            .unwrap();

        assert_eq!(output["snippets"].as_array().unwrap().len(), 2);
        assert_eq!(output["snippets"][0]["path"], json!("src/lib.rs"));
        assert_eq!(output["snippets"][0]["start_line"], json!(2));
        assert_eq!(output["snippets"][0]["end_line"], json!(4));
        assert!(output["artifacts"][0]["content"]
            .as_str()
            .unwrap()
            .contains("--- src/lib.rs:2-4 ---"));
        assert!(output["artifacts"][0]["content"]
            .as_str()
            .unwrap()
            .contains("3: fn alpha() {}"));
        assert_eq!(output["artifacts"][0]["kind"], json!("code_context"));
        assert_eq!(tools.tool_capability("repo.context"), Some("code.read"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn repo_context_supports_explicit_regex_mode() {
        let dir = temp_dir("air-tools-repo-context-regex");
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("src/lib.rs"),
            "line 1\nfn alpha_value() {}\nline 3\nfn betaValue() {}\n",
        )
        .unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "repo.context": {
                  "kind": "repo_context",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_matches": 10,
                  "max_files": 2,
                  "context_lines": 1,
                  "max_bytes": 4096
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool(
                "repo.context",
                &json!({"query": "fn [a-z]+_value", "mode": "regex", "path": "src/lib.rs"}),
            )
            .unwrap();

        assert_eq!(output["mode"], json!("regex"));
        assert_eq!(output["matches"].as_array().unwrap().len(), 1);
        assert_eq!(output["snippets"][0]["path"], json!("src/lib.rs"));
        assert!(output["snippets"][0]["content"]
            .as_str()
            .unwrap()
            .contains("alpha_value"));
        assert_eq!(output["artifacts"][0]["metadata"]["mode"], json!("regex"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn repo_context_rejects_unknown_mode() {
        let dir = temp_dir("air-tools-repo-context-mode");
        fs::write(dir.join("lib.rs"), "fn alpha() {}\n").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "repo.context": {
                  "kind": "repo_context",
                  "capability": "code.read",
                  "repo_dir": "."
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let error = tools
            .call_tool("repo.context", &json!({"query": "alpha", "mode": "glob"}))
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("input.mode must be fixed or regex"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn repo_symbols_returns_lightweight_symbol_map() {
        let dir = temp_dir("air-tools-repo-symbols");
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("src/lib.rs"),
            "pub struct Alpha {}\nfn beta_value() {}\nlet gamma = 1;\n",
        )
        .unwrap();
        fs::write(
            dir.join("src/app.ts"),
            "export function renderView() {}\nclass Panel {}\n",
        )
        .unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "repo.symbols": {
                  "kind": "repo_symbols",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_symbols": 10,
                  "max_bytes": 4096
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool("repo.symbols", &json!({"query": "alpha"}))
            .unwrap();

        assert_eq!(output["query"], json!("alpha"));
        assert_eq!(output["symbols"].as_array().unwrap().len(), 1);
        assert_eq!(output["symbols"][0]["kind"], json!("struct"));
        assert_eq!(output["symbols"][0]["name"], json!("Alpha"));
        assert_eq!(output["artifacts"][0]["kind"], json!("repo_symbols"));
        assert_eq!(tools.tool_capability("repo.symbols"), Some("code.read"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn repo_symbols_supports_path_and_glob_filters() {
        let dir = temp_dir("air-tools-repo-symbols-filters");
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/lib.rs"), "pub struct Alpha {}\n").unwrap();
        fs::write(dir.join("src/app.ts"), "export function renderView() {}\n").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "repo.symbols": {
                  "kind": "repo_symbols",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_symbols": 10
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool(
                "repo.symbols",
                &json!({"path": "src", "glob": "*.ts", "max_symbols": 2}),
            )
            .unwrap();

        assert_eq!(output["symbols"].as_array().unwrap().len(), 1);
        assert_eq!(output["symbols"][0]["path"], json!("src/app.ts"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn repo_references_returns_definition_references_and_snippets() {
        let dir = temp_dir("air-tools-repo-references");
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("src/lib.rs"),
            "pub struct Alpha {}\nimpl Alpha { fn new() -> Alpha { Alpha {} } }\nlet Alphabet = 1;\n",
        )
        .unwrap();
        fs::write(
            dir.join("src/app.ts"),
            "function useAlpha(value: Alpha) { return value }\n",
        )
        .unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "repo.references": {
                  "kind": "repo_references",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_matches": 10,
                  "max_files": 4,
                  "context_lines": 1,
                  "max_bytes": 4096
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool("repo.references", &json!({"symbol": "Alpha"}))
            .unwrap();

        assert_eq!(output["symbol"], json!("Alpha"));
        assert_eq!(output["definitions"].as_array().unwrap().len(), 1);
        assert_eq!(output["definitions"][0]["kind"], json!("struct"));
        assert_eq!(output["references"].as_array().unwrap().len(), 3);
        assert!(output["references"]
            .as_array()
            .unwrap()
            .iter()
            .all(|reference| reference["text"] != json!("let Alphabet = 1;")));
        assert!(!output["snippets"].as_array().unwrap().is_empty());
        assert_eq!(output["artifacts"][0]["kind"], json!("repo_references"));
        assert_eq!(tools.tool_capability("repo.references"), Some("code.read"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn repo_references_rejects_non_identifier_symbols() {
        let dir = temp_dir("air-tools-repo-references-invalid");
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "repo.references": {
                  "kind": "repo_references",
                  "repo_dir": "."
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let error = tools
            .call_tool("repo.references", &json!({"symbol": "Alpha Beta"}))
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("input.symbol must be an identifier-like token"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn diagnostic_context_returns_source_snippets_for_command_diagnostics() {
        let dir = temp_dir("air-tools-diagnostic-context");
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("src/lib.rs"),
            "line 1\nline 2\nfn broken() {}\nline 4\nline 5\n",
        )
        .unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "diagnostic.context": {
                  "kind": "diagnostic_context",
                  "capability": "code.read",
                  "repo_dir": ".",
                  "max_diagnostics": 5,
                  "context_lines": 1,
                  "max_bytes": 4096
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool(
                "diagnostic.context",
                &json!({
                    "diagnostics": [{
                        "source": "command_run",
                        "severity": "error",
                        "path": "src/lib.rs",
                        "line": 3,
                        "column": 4,
                        "message": "expected value"
                    }]
                }),
            )
            .unwrap();

        assert_eq!(output["snippets"].as_array().unwrap().len(), 1);
        assert_eq!(output["snippets"][0]["path"], json!("src/lib.rs"));
        assert_eq!(output["snippets"][0]["start_line"], json!(2));
        assert_eq!(output["snippets"][0]["end_line"], json!(4));
        assert!(output["snippets"][0]["content"]
            .as_str()
            .unwrap()
            .contains("3: fn broken() {}"));
        assert_eq!(output["artifacts"][0]["kind"], json!("diagnostic_context"));
        assert_eq!(
            output["artifacts"][0]["metadata"]["provider"],
            json!("diagnostic_context")
        );
        assert!(output["unreadable"].as_array().unwrap().is_empty());
        assert_eq!(
            tools.tool_capability("diagnostic.context"),
            Some("code.read")
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn diagnostic_context_skips_paths_outside_repo() {
        let dir = temp_dir("air-tools-diagnostic-context-boundary");
        fs::write(dir.join("lib.rs"), "fn alpha() {}\n").unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "diagnostic.context": {
                  "kind": "diagnostic_context",
                  "capability": "code.read",
                  "repo_dir": "."
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool(
                "diagnostic.context",
                &json!({
                    "diagnostics": [{
                        "path": "../secret.txt",
                        "line": 1
                    }]
                }),
            )
            .unwrap();

        assert!(output["snippets"].as_array().unwrap().is_empty());
        assert_eq!(output["unreadable"].as_array().unwrap().len(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn command_run_executes_allowlisted_command() {
        let dir = temp_dir("air-tools-command-run");
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "test.run": {
                  "kind": "command_run",
                  "capability": "code.test",
                  "cwd": ".",
                  "commands": {
                    "cargo_version": ["cargo", "--version"]
                  },
                  "timeout_seconds": 10
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool("test.run", &json!({"command": "cargo_version"}))
            .unwrap();

        assert_eq!(output["success"], json!(true));
        assert!(output["log"].as_str().unwrap().contains("cargo"));
        assert_eq!(output["artifacts"][0]["kind"], json!("test_log"));
        assert_eq!(tools.tool_capability("test.run"), Some("code.test"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn command_run_extracts_structured_diagnostics() {
        let dir = temp_dir("air-tools-command-run-diagnostics");
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "test.run": {
                  "kind": "command_run",
                  "capability": "code.test",
                  "cwd": ".",
                  "commands": {
                    "tsc": [
                      "node",
                      "-e",
                      "console.error('src/main.ts(4,9): error TS2304: Cannot find name x.'); process.exit(2)"
                    ]
                  },
                  "timeout_seconds": 10
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool("test.run", &json!({"command": "tsc"}))
            .unwrap();

        assert_eq!(output["success"], json!(false));
        assert_eq!(output["diagnostics"][0]["path"], json!("src/main.ts"));
        assert_eq!(output["diagnostics"][0]["line"], json!(4));
        assert_eq!(output["diagnostics"][0]["column"], json!(9));
        assert_eq!(output["diagnostics"][0]["severity"], json!("error"));
        assert!(output["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("TS2304"));
        assert_eq!(
            output["artifacts"][0]["metadata"]["diagnostics_count"],
            json!(1)
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn extract_command_diagnostics_parses_rustc_location_blocks() {
        let diagnostics = extract_command_diagnostics(
            "error[E0425]: cannot find value `missing` in this scope\n  --> src/lib.rs:12:5\n",
            10,
        );

        assert_eq!(diagnostics[0]["severity"], json!("error"));
        assert_eq!(diagnostics[0]["path"], json!("src/lib.rs"));
        assert_eq!(diagnostics[0]["line"], json!(12));
        assert_eq!(diagnostics[0]["column"], json!(5));
        assert!(diagnostics[0]["message"]
            .as_str()
            .unwrap()
            .contains("cannot find value"));
    }

    #[test]
    fn extract_command_diagnostics_parses_python_tracebacks() {
        let diagnostics = extract_command_diagnostics(
            r#"Traceback (most recent call last):
  File "/tmp/project/run.py", line 8, in <module>
    main()
  File "src/app.py", line 3, in main
    assert False
AssertionError: broken invariant
"#,
            10,
        );

        assert_eq!(diagnostics[0]["severity"], json!("error"));
        assert_eq!(diagnostics[0]["path"], json!("src/app.py"));
        assert_eq!(diagnostics[0]["line"], json!(3));
        assert_eq!(diagnostics[0]["column"], json!(1));
        assert_eq!(
            diagnostics[0]["message"],
            json!("AssertionError: broken invariant")
        );
    }

    #[test]
    fn extract_command_diagnostics_parses_line_only_colon_diagnostics() {
        let diagnostics = extract_command_diagnostics(
            "src/app.py:12: error: Incompatible return value type\n\
             tests/test_app.py:7: AssertionError: expected true\n\
             src/app.py:12: in handler\n",
            10,
        );

        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0]["severity"], json!("error"));
        assert_eq!(diagnostics[0]["path"], json!("src/app.py"));
        assert_eq!(diagnostics[0]["line"], json!(12));
        assert_eq!(diagnostics[0]["column"], json!(1));
        assert_eq!(
            diagnostics[0]["message"],
            json!("Incompatible return value type")
        );
        assert_eq!(diagnostics[1]["severity"], json!("error"));
        assert_eq!(diagnostics[1]["path"], json!("tests/test_app.py"));
        assert_eq!(diagnostics[1]["line"], json!(7));
        assert_eq!(
            diagnostics[1]["message"],
            json!("AssertionError: expected true")
        );
    }

    #[test]
    fn extract_command_diagnostics_parses_file_context_lint_blocks() {
        let diagnostics = extract_command_diagnostics(
            r#"src/main.ts
  12:5  error  Unexpected any.  @typescript-eslint/no-explicit-any
  18:1  warning  Missing return type  @typescript-eslint/explicit-function-return-type
✖ 2 problems
"#,
            10,
        );

        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0]["severity"], json!("error"));
        assert_eq!(diagnostics[0]["path"], json!("src/main.ts"));
        assert_eq!(diagnostics[0]["line"], json!(12));
        assert_eq!(diagnostics[0]["column"], json!(5));
        assert_eq!(
            diagnostics[0]["message"],
            json!("Unexpected any.  @typescript-eslint/no-explicit-any")
        );
        assert_eq!(diagnostics[1]["severity"], json!("warning"));
        assert_eq!(diagnostics[1]["line"], json!(18));
        assert_eq!(diagnostics[1]["column"], json!(1));
    }

    #[test]
    fn local_docs_search_returns_artifacts_for_documents() {
        let output = search_docs(
            &json!({"query": "provenance"}),
            &[LocalDoc {
                id: "doc-1".to_string(),
                title: "Provenance".to_string(),
                content: "AIR provenance artifacts".to_string(),
            }],
            3,
        );

        assert_eq!(output["documents"][0]["id"], json!("doc-1"));
        assert_eq!(output["artifacts"][0]["id"], json!("doc-1"));
        assert_eq!(output["artifacts"][0]["kind"], json!("doc_chunk"));
        assert_eq!(output["artifacts"][0]["uri"], json!("local-doc://doc-1"));
    }

    #[test]
    fn playwright_search_invokes_script_and_returns_artifacts() {
        let dir = temp_dir("air-tools-playwright-search");
        fs::write(
            dir.join("search.cjs"),
            r#"
const chunks = [];
process.stdin.on('data', chunk => chunks.push(chunk));
process.stdin.on('end', () => {
  const input = JSON.parse(Buffer.concat(chunks).toString('utf8'));
  process.stdout.write(JSON.stringify({
    query: input.query,
    received: input,
    documents: [{ id: 'web:example', title: 'Example', url: 'https://example.com', content: 'Example content' }],
    artifacts: [{ id: 'web:example', kind: 'web_page', title: 'Example', uri: 'https://example.com', content: 'Example content', metadata: { provider: 'playwright_search' } }]
  }));
});
"#,
        )
        .unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "web.search": {
                  "kind": "playwright_search",
                  "capability": "network.search",
                  "script_path": "search.cjs",
                  "query_variants": ["{{query}} GitHub", "{{query}} docs"],
                  "max_results": 2,
                  "max_results_per_query": 4,
                  "max_per_domain": 1,
                  "max_content_chars": 1000,
                  "include_domains": ["github.com", "openclaw.ai"],
                  "exclude_domains": ["example-spam.test"],
                  "required_terms": ["openclaw"],
                  "exclude_terms": ["spam"],
                  "page_concurrency": 2,
                  "navigation_timeout_ms": 1000,
                  "overall_timeout_ms": 4000,
                  "search_delay_ms": 100,
                  "retry_count": 1,
                  "user_agent": "AIR test",
                  "fetch_pages": false,
                  "cache_dir": "cache",
                  "cache_ttl_seconds": 3600,
                  "timeout_seconds": 5
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool("web.search", &json!({"query": "openclaw"}))
            .unwrap();

        assert_eq!(output["query"], json!("openclaw"));
        assert_eq!(
            output["received"]["query_variants"],
            json!(["openclaw GitHub", "openclaw docs"])
        );
        assert_eq!(output["received"]["max_results_per_query"], json!(4));
        assert_eq!(output["received"]["max_per_domain"], json!(1));
        assert_eq!(
            output["received"]["include_domains"],
            json!(["github.com", "openclaw.ai"])
        );
        assert_eq!(
            output["received"]["exclude_domains"],
            json!(["example-spam.test"])
        );
        assert_eq!(output["received"]["required_terms"], json!(["openclaw"]));
        assert_eq!(output["received"]["exclude_terms"], json!(["spam"]));
        assert_eq!(output["received"]["page_concurrency"], json!(2));
        assert_eq!(output["received"]["overall_timeout_ms"], json!(4000));
        assert_eq!(output["received"]["search_delay_ms"], json!(100));
        assert_eq!(output["received"]["retry_count"], json!(1));
        assert_eq!(output["received"]["user_agent"], json!("AIR test"));
        assert_eq!(output["received"]["fetch_pages"], json!(false));
        assert!(output["received"]["cache_dir"]
            .as_str()
            .unwrap()
            .ends_with("cache"));
        assert_eq!(output["received"]["cache_ttl_seconds"], json!(3600));
        assert_eq!(output["artifacts"][0]["id"], json!("web:example"));
        assert_eq!(tools.tool_capability("web.search"), Some("network.search"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn playwright_page_audit_invokes_script_and_bounds_paths() {
        let dir = temp_dir("air-tools-playwright-page-audit");
        fs::write(
            dir.join("index.html"),
            "<!doctype html><html><body>ok</body></html>",
        )
        .unwrap();
        fs::write(
            dir.join("audit.cjs"),
            r#"
const chunks = [];
process.stdin.on('data', chunk => chunks.push(chunk));
process.stdin.on('end', () => {
  const input = JSON.parse(Buffer.concat(chunks).toString('utf8'));
  process.stdout.write(JSON.stringify({
    target: input.path || input.url,
    received: input,
    success: true,
    viewport_count: input.viewports.length,
    viewports: [{ width: input.viewports[0].width, height: input.viewports[0].height, horizontal_overflow: false, overlap_count: 0, screenshot_path: input.screenshot_dir + '/page.png' }],
    diagnostics: [],
    artifacts: [{ id: 'browser-screenshot:test', kind: 'browser_screenshot', title: 'audit', uri: input.screenshot_dir + '/page.png', content: '', metadata: { provider: 'playwright_page_audit' } }]
  }));
});
"#,
        )
        .unwrap();
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "browser.audit": {
                  "kind": "playwright_page_audit",
                  "capability": "browser.audit",
                  "script_path": "audit.cjs",
                  "base_dir": ".",
                  "screenshot_dir": "screens",
                  "viewports": [{ "label": "desktop", "width": 1280, "height": 900 }],
                  "navigation_timeout_ms": 1000,
                  "max_text_chars": 500,
                  "timeout_seconds": 5
                }
              }
            }"#,
        );
        let mut tools = ConfigTools::from_file(config_path).unwrap();

        let output = tools
            .call_tool("browser.audit", &json!({"path": "index.html"}))
            .unwrap();

        assert_eq!(output["success"], json!(true));
        assert_eq!(output["viewport_count"], json!(1));
        assert!(output["received"]["path"]
            .as_str()
            .unwrap()
            .ends_with("index.html"));
        assert_eq!(output["received"]["viewports"][0]["width"], json!(1280));
        assert!(output["received"]["screenshot_dir"]
            .as_str()
            .unwrap()
            .ends_with("screens"));
        assert_eq!(output["artifacts"][0]["kind"], json!("browser_screenshot"));
        assert_eq!(
            tools.tool_capability("browser.audit"),
            Some("browser.audit")
        );

        let error = tools
            .call_tool("browser.audit", &json!({"path": "../outside.html"}))
            .unwrap_err();
        assert!(error.to_string().contains("input.path"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn web_fetch_config_validates_limits() {
        let dir = temp_dir("air-tools-web-fetch-config");
        let config_path = write_config(
            &dir,
            r#"{
              "tools": {
                "web.fetch": {
                  "kind": "web_fetch",
                  "capability": "network.fetch",
                  "timeout_seconds": 0
                }
              }
            }"#,
        );

        let error = ConfigTools::from_file(config_path).unwrap_err();

        assert!(error.to_string().contains("timeout_seconds"));
        assert!(error.to_string().contains("greater than 0"));
        let _ = fs::remove_dir_all(dir);
    }
}
