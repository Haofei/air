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

mod file_tools;
use file_tools::{
    call_file_edit_tool, call_file_ops_tool, call_file_patch_tool, call_file_read_many_tool,
    call_file_read_tool, call_file_search_tool, call_file_write_tool, file_modified_time,
    is_likely_binary, FileEditOptions, FileOpsOptions, FilePatchOptions, FileWriteOptions,
};
mod helpdesk;
use helpdesk::helpdesk_docs;
mod provider;
pub use provider::{EchoTools, ToolProviderChoice};

const DEFAULT_CONTEXT_MAX_CHARS: usize = 200_000;
const DEFAULT_CONTEXT_THRESHOLD_PERCENT: u64 = 80;

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
        required_text: Option<Vec<String>>,

        #[serde(default)]
        forbidden_text: Option<Vec<String>>,

        #[serde(default)]
        require_canvas: Option<bool>,

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
    FileSearch {
        #[serde(default)]
        capability: Option<String>,

        base_dir: PathBuf,

        #[serde(default)]
        max_bytes: Option<usize>,

        #[serde(default)]
        max_matches: Option<usize>,

        #[serde(default)]
        max_context_lines: Option<usize>,

        #[serde(default)]
        max_line_chars: Option<usize>,
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
        max_changed_lines: Option<usize>,

        #[serde(default)]
        require_read: Option<bool>,

        #[serde(default)]
        allow_replace_all: Option<bool>,
    },
    FileOps {
        #[serde(default)]
        capability: Option<String>,

        base_dir: PathBuf,

        #[serde(default)]
        max_bytes: Option<usize>,

        #[serde(default)]
        max_files: Option<usize>,

        #[serde(default)]
        max_changed_lines: Option<usize>,

        #[serde(default)]
        require_read: Option<bool>,

        #[serde(default)]
        allow_new_files: Option<bool>,

        #[serde(default)]
        allow_overwrite: Option<bool>,

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
        max_changed_lines: Option<usize>,

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
    ArtifactValidate {
        #[serde(default)]
        capability: Option<String>,

        #[serde(default)]
        fail_on_missing: Option<bool>,

        #[serde(default)]
        max_ids: Option<usize>,
    },
    CandidateValidate {
        #[serde(default)]
        capability: Option<String>,

        base_dir: PathBuf,

        allowed_test_commands: Vec<String>,
    },
    CommandRun {
        #[serde(default)]
        capability: Option<String>,

        cwd: PathBuf,

        commands: BTreeMap<String, Vec<String>>,

        #[serde(default)]
        parameters: BTreeMap<String, CommandParameterRule>,

        #[serde(default)]
        timeout_seconds: Option<u64>,

        #[serde(default)]
        max_bytes: Option<usize>,

        #[serde(default)]
        truncation_direction: Option<TruncationDirection>,
    },
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TruncationDirection {
    Head,
    Tail,
}

#[derive(Debug, Clone, Copy)]
struct CommandRunOptions {
    timeout_seconds: u64,
    max_bytes: usize,
    truncation_direction: TruncationDirection,
}

#[derive(Debug, Clone, Deserialize)]
struct CommandParameterRule {
    #[serde(default)]
    values: Option<Vec<String>>,

    #[serde(default)]
    max_chars: Option<usize>,

    #[serde(default)]
    allow: CommandParameterAllow,
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum CommandParameterAllow {
    Identifier,
    Path,
    #[default]
    SafeArg,
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
            | ToolConfig::FileSearch { capability, .. }
            | ToolConfig::FileWrite { capability, .. }
            | ToolConfig::FileEdit { capability, .. }
            | ToolConfig::FileOps { capability, .. }
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
            | ToolConfig::ArtifactValidate { capability, .. }
            | ToolConfig::CandidateValidate { capability, .. }
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

    fn remember_repo_output_paths(
        &mut self,
        repo_dir: &Path,
        output: &Value,
    ) -> Result<(), RuntimeError> {
        for field in ["matches", "snippets", "references", "definitions"] {
            for path in output
                .get(field)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|item| item.get("path").and_then(Value::as_str))
            {
                if path.is_empty() {
                    continue;
                }
                self.remember_read_snapshot(&repo_dir.join(path))?;
            }
        }
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
                required_text,
                forbidden_text,
                require_canvas: _,
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
                validate_non_empty_strings(
                    path,
                    &format!("tools.{name}.required_text"),
                    required_text.as_deref(),
                )?;
                validate_non_empty_strings(
                    path,
                    &format!("tools.{name}.forbidden_text"),
                    forbidden_text.as_deref(),
                )?;
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
            ToolConfig::FileSearch {
                base_dir,
                max_bytes,
                max_matches,
                max_context_lines,
                max_line_chars,
                ..
            } => {
                if base_dir.as_os_str().is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.base_dir must not be empty",
                        path.display()
                    );
                }
                validate_positive_usize(path, &format!("tools.{name}.max_bytes"), *max_bytes)?;
                validate_positive_usize(path, &format!("tools.{name}.max_matches"), *max_matches)?;
                validate_positive_usize(
                    path,
                    &format!("tools.{name}.max_context_lines"),
                    *max_context_lines,
                )?;
                validate_positive_usize(
                    path,
                    &format!("tools.{name}.max_line_chars"),
                    *max_line_chars,
                )?;
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
                max_changed_lines,
                ..
            } => {
                if base_dir.as_os_str().is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.base_dir must not be empty",
                        path.display()
                    );
                }
                validate_positive_usize(path, &format!("tools.{name}.max_bytes"), *max_bytes)?;
                validate_positive_usize(
                    path,
                    &format!("tools.{name}.max_changed_lines"),
                    *max_changed_lines,
                )?;
            }
            ToolConfig::FileOps {
                base_dir,
                max_bytes,
                max_files,
                max_changed_lines,
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
                validate_positive_usize(
                    path,
                    &format!("tools.{name}.max_changed_lines"),
                    *max_changed_lines,
                )?;
            }
            ToolConfig::FilePatch {
                repo_dir,
                max_bytes,
                max_files,
                max_changed_lines,
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
                validate_positive_usize(
                    path,
                    &format!("tools.{name}.max_changed_lines"),
                    *max_changed_lines,
                )?;
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
            ToolConfig::ArtifactValidate { max_ids, .. } => {
                validate_positive_usize(path, &format!("tools.{name}.max_ids"), *max_ids)?;
            }
            ToolConfig::CandidateValidate {
                base_dir,
                allowed_test_commands,
                ..
            } => {
                if base_dir.as_os_str().is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.base_dir must not be empty",
                        path.display()
                    );
                }
                if allowed_test_commands.is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.allowed_test_commands must not be empty",
                        path.display()
                    );
                }
                validate_non_empty_strings(
                    path,
                    &format!("tools.{name}.allowed_test_commands"),
                    Some(allowed_test_commands),
                )?;
            }
            ToolConfig::CommandRun {
                cwd,
                commands,
                parameters,
                timeout_seconds,
                max_bytes,
                truncation_direction: _,
                ..
            } => {
                if cwd.as_os_str().is_empty() {
                    anyhow::bail!(
                        "tool config {} tools.{name}.cwd must not be empty",
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
                for (parameter, rule) in parameters {
                    if parameter.trim().is_empty() {
                        anyhow::bail!(
                            "tool config {} tools.{name}.parameters contains an empty parameter name",
                            path.display()
                        );
                    }
                    validate_positive_usize(
                        path,
                        &format!("tools.{name}.parameters.{parameter}.max_chars"),
                        rule.max_chars,
                    )?;
                    if rule.values.as_ref().is_some_and(Vec::is_empty) {
                        anyhow::bail!(
                            "tool config {} tools.{name}.parameters.{parameter}.values must not be empty",
                            path.display()
                        );
                    }
                    if let Some(values) = &rule.values {
                        for value in values {
                            if value.is_empty() {
                                anyhow::bail!(
                                    "tool config {} tools.{name}.parameters.{parameter}.values must contain non-empty strings",
                                    path.display()
                                );
                            }
                            validate_command_parameter_value(name, parameter, value, rule)
                                .map_err(|error| anyhow::anyhow!("{error}"))?;
                        }
                    }
                }
                for (alias, command) in commands {
                    for part in command {
                        for parameter in command_template_parameters(part) {
                            if !parameters.contains_key(&parameter) {
                                anyhow::bail!(
                                    "tool config {} tools.{name}.commands.{alias} references parameter {parameter}, but tools.{name}.parameters.{parameter} is not declared",
                                    path.display()
                                );
                            }
                        }
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
                required_text,
                forbidden_text,
                require_canvas,
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
                    required_text: required_text.as_deref(),
                    forbidden_text: forbidden_text.as_deref(),
                    require_canvas,
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
            ToolConfig::FileSearch {
                capability: _,
                base_dir,
                max_bytes,
                max_matches,
                max_context_lines,
                max_line_chars,
            } => {
                let base_dir = resolve_config_path(&self.config_dir, &base_dir);
                let output = call_file_search_tool(
                    name,
                    input,
                    &base_dir,
                    max_bytes.unwrap_or(64 * 1024),
                    max_matches.unwrap_or(100),
                    max_context_lines.unwrap_or(8),
                    max_line_chars.unwrap_or(2000),
                )?;
                if let Some(path) = output.get("path").and_then(Value::as_str) {
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
                max_changed_lines,
                require_read,
                allow_replace_all,
            } => {
                let output = call_file_edit_tool(
                    name,
                    input,
                    FileEditOptions {
                        base_dir: &resolve_config_path(&self.config_dir, &base_dir),
                        max_bytes: max_bytes.unwrap_or(256 * 1024),
                        max_changed_lines,
                        require_read: require_read.unwrap_or(true),
                        allow_replace_all: allow_replace_all.unwrap_or(false),
                        read_snapshots: &self.read_snapshots,
                    },
                )?;
                if let Some(path) = output.get("path").and_then(Value::as_str) {
                    self.remember_read_snapshot(Path::new(path))?;
                }
                Ok(output)
            }
            ToolConfig::FileOps {
                capability: _,
                base_dir,
                max_bytes,
                max_files,
                max_changed_lines,
                require_read,
                allow_new_files,
                allow_overwrite,
                allow_replace_all,
            } => {
                let base_dir = resolve_config_path(&self.config_dir, &base_dir);
                let output = call_file_ops_tool(
                    name,
                    input,
                    FileOpsOptions {
                        base_dir: &base_dir,
                        max_bytes: max_bytes.unwrap_or(256 * 1024),
                        max_files: max_files.unwrap_or(8),
                        max_changed_lines,
                        require_read: require_read.unwrap_or(true),
                        allow_new_files: allow_new_files.unwrap_or(false),
                        allow_overwrite: allow_overwrite.unwrap_or(false),
                        allow_replace_all: allow_replace_all.unwrap_or(false),
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
                            if let Some(path) = file.get("path").and_then(Value::as_str) {
                                self.remember_read_snapshot(&base_dir.join(path))?;
                            }
                        }
                    }
                }
                Ok(output)
            }
            ToolConfig::FilePatch {
                capability: _,
                repo_dir,
                max_bytes,
                max_files,
                max_changed_lines,
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
                        max_changed_lines,
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
            } => {
                let repo_dir = resolve_config_path(&self.config_dir, &repo_dir);
                let output = call_repo_search_tool(
                    name,
                    input,
                    &repo_dir,
                    max_matches.unwrap_or(80),
                    max_bytes.unwrap_or(256 * 1024),
                )?;
                self.remember_repo_output_paths(&repo_dir, &output)?;
                Ok(output)
            }
            ToolConfig::RepoContext {
                capability: _,
                repo_dir,
                max_matches,
                max_files,
                context_lines,
                max_bytes,
            } => {
                let repo_dir = resolve_config_path(&self.config_dir, &repo_dir);
                let output = call_repo_context_tool(
                    name,
                    input,
                    &repo_dir,
                    max_matches.unwrap_or(40),
                    max_files.unwrap_or(8),
                    context_lines.unwrap_or(6),
                    max_bytes.unwrap_or(256 * 1024),
                )?;
                self.remember_repo_output_paths(&repo_dir, &output)?;
                Ok(output)
            }
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
            } => {
                let repo_dir = resolve_config_path(&self.config_dir, &repo_dir);
                let output = call_repo_references_tool(
                    name,
                    input,
                    &repo_dir,
                    max_matches.unwrap_or(120),
                    max_files.unwrap_or(12),
                    context_lines.unwrap_or(4),
                    max_bytes.unwrap_or(256 * 1024),
                )?;
                self.remember_repo_output_paths(&repo_dir, &output)?;
                Ok(output)
            }
            ToolConfig::DiagnosticContext {
                capability: _,
                repo_dir,
                max_diagnostics,
                context_lines,
                max_bytes,
            } => {
                let repo_dir = resolve_config_path(&self.config_dir, &repo_dir);
                let output = call_diagnostic_context_tool(
                    name,
                    input,
                    &repo_dir,
                    max_diagnostics.unwrap_or(20),
                    context_lines.unwrap_or(4),
                    max_bytes.unwrap_or(256 * 1024),
                )?;
                self.remember_repo_output_paths(&repo_dir, &output)?;
                Ok(output)
            }
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
                max_context_chars.unwrap_or(DEFAULT_CONTEXT_MAX_CHARS),
                threshold_percent.unwrap_or(DEFAULT_CONTEXT_THRESHOLD_PERCENT),
            ),
            ToolConfig::ArtifactValidate {
                capability: _,
                fail_on_missing,
                max_ids,
            } => call_artifact_validate_tool(
                name,
                input,
                fail_on_missing.unwrap_or(false),
                max_ids.unwrap_or(512),
            ),
            ToolConfig::CandidateValidate {
                capability: _,
                base_dir,
                allowed_test_commands,
            } => call_candidate_validate_tool(
                name,
                input,
                &resolve_config_path(&self.config_dir, &base_dir),
                &allowed_test_commands,
            ),
            ToolConfig::CommandRun {
                capability: _,
                cwd,
                commands,
                parameters,
                timeout_seconds,
                max_bytes,
                truncation_direction,
            } => call_command_run_tool(
                name,
                input,
                &resolve_config_path(&self.config_dir, &cwd),
                &commands,
                &parameters,
                CommandRunOptions {
                    timeout_seconds: timeout_seconds.unwrap_or(120),
                    max_bytes: max_bytes.unwrap_or(256 * 1024),
                    truncation_direction: truncation_direction.unwrap_or(TruncationDirection::Head),
                },
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
            | ToolConfig::FileSearch { capability, .. }
            | ToolConfig::FileWrite { capability, .. }
            | ToolConfig::FileEdit { capability, .. }
            | ToolConfig::FileOps { capability, .. }
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
            | ToolConfig::ArtifactValidate { capability, .. }
            | ToolConfig::CandidateValidate { capability, .. }
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
    required_text: Option<&'a [String]>,
    forbidden_text: Option<&'a [String]>,
    require_canvas: Option<bool>,
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

fn call_git_diff_tool(
    name: &str,
    input: &Value,
    repo_dir: &Path,
    max_bytes: usize,
) -> Result<Value, RuntimeError> {
    let repo = canonicalize_tool_path(name, "repo_dir", repo_dir)?;
    let (paths, has_filter) = git_diff_paths(name, input)?;
    if has_filter && paths.is_empty() {
        return Ok(git_diff_output(
            &repo,
            input,
            &paths,
            &[],
            String::new(),
            false,
            0,
        ));
    }
    let mut command = Command::new("git");
    command.arg("-C").arg(&repo).arg("diff");
    if input
        .get("staged")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        command.arg("--staged");
    }
    if !paths.is_empty() {
        command.arg("--");
        command.args(&paths);
    }
    let staged = input
        .get("staged")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let output = command
        .output()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} git diff: {error}")))?;
    if !output.status.success() {
        return Err(RuntimeError::Provider(format!(
            "tool {name} git diff failed: {}",
            provider_error_snippet(&String::from_utf8_lossy(&output.stderr))
        )));
    }
    let mut raw_diff = String::from_utf8_lossy(&output.stdout).to_string();
    let untracked_files = if !staged && has_filter && !paths.is_empty() {
        append_requested_untracked_diffs(name, &repo, &paths, &mut raw_diff)?
    } else {
        Vec::new()
    };
    let (diff, truncated, bytes) = bytes_to_limited_text(raw_diff.as_bytes(), max_bytes);
    Ok(git_diff_output(
        &repo,
        input,
        &paths,
        &untracked_files,
        diff,
        truncated,
        bytes,
    ))
}

fn git_diff_output(
    repo: &Path,
    input: &Value,
    paths: &[String],
    untracked_files: &[String],
    diff: String,
    truncated: bool,
    bytes: usize,
) -> Value {
    let artifact_id = format!(
        "git-diff:{}:{}:{}",
        repo.display(),
        input
            .get("staged")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        paths.join(",")
    );
    json!({
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
                "paths": paths,
                "untracked_files": untracked_files,
                "bytes": bytes,
                "truncated": truncated
            }
        }],
    })
}

fn append_requested_untracked_diffs(
    name: &str,
    repo: &Path,
    paths: &[String],
    diff: &mut String,
) -> Result<Vec<String>, RuntimeError> {
    let mut included = Vec::new();
    for path in paths {
        let candidate = repo.join(path);
        let Ok(absolute_path) = canonicalize_tool_path(name, "git diff path", &candidate) else {
            continue;
        };
        if !absolute_path.starts_with(repo) || !absolute_path.is_file() {
            continue;
        }
        if git_path_is_tracked(name, repo, path)? {
            continue;
        }
        let body = fs::read(&absolute_path)
            .map_err(|error| RuntimeError::Provider(format!("tool {name} read file: {error}")))?;
        if is_likely_binary(&body) {
            continue;
        }
        let content = std::str::from_utf8(&body).map_err(|_| {
            RuntimeError::Provider(format!(
                "tool {name} requested untracked file {path} is not valid UTF-8"
            ))
        })?;
        if !diff.is_empty() && !diff.ends_with('\n') {
            diff.push('\n');
        }
        diff.push_str(&untracked_file_unified_diff(path, content));
        included.push(path.clone());
    }
    Ok(included)
}

fn git_path_is_tracked(name: &str, repo: &Path, path: &str) -> Result<bool, RuntimeError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(path)
        .output()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} git ls-files: {error}")))?;
    Ok(output.status.success())
}

fn untracked_file_unified_diff(path: &str, content: &str) -> String {
    let lines = content.lines().collect::<Vec<_>>();
    let mut diff = format!(
        "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{} @@\n",
        lines.len()
    );
    for line in lines {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    diff
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
    let paths = repo_tool_paths(name, input)?;
    let glob_input = repo_files_glob_input(name, input, query_input.pattern_glob)?;
    let mut command = Command::new("rg");
    command.arg("--files");
    if let Some(glob) = glob_input.glob {
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
    let content = files.join("\n");
    Ok(json!({
        "repo": repo.display().to_string(),
        "query": query_input.raw_query,
        "query_source": query_input.query_source,
        "effective_query": effective_query,
        "glob": glob_input.glob,
        "glob_source": glob_input.glob_source,
        "mode": mode,
        "files": files,
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
                "include_all": include_all
            }
        }]
    }))
}

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
        return query
            .as_str()
            .map(|q| RepoFilesQueryInput {
                raw_query: q.trim(),
                query_source: "query",
                pattern_glob: None,
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
    if let Some(glob) = input.get("glob") {
        return glob
            .as_str()
            .map(|g| RepoFilesGlobInput {
                glob: Some(g),
                glob_source: "glob",
            })
            .ok_or_else(|| {
                RuntimeError::Provider(format!("tool {tool_name} input.glob must be a string"))
            });
    }
    if let Some(glob) = input.get("file_glob") {
        return glob
            .as_str()
            .map(|g| RepoFilesGlobInput {
                glob: Some(g),
                glob_source: "file_glob",
            })
            .ok_or_else(|| {
                RuntimeError::Provider(format!("tool {tool_name} input.file_glob must be a string"))
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

fn call_repo_search_tool(
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
    let mut mode = requested_mode.to_string();
    let mut effective_query = repo_effective_search_query(&mode, query);
    let effective_max_matches =
        optional_bounded_usize_input(name, input, "max_matches", max_matches)?
            .unwrap_or(max_matches);
    let repo = canonicalize_tool_path(name, "repo_dir", repo_dir)?;
    let paths = repo_tool_paths(name, input)?;
    let glob = repo_glob_input(name, input)?;
    if let Some(glob) = glob {
        validate_git_pathspec(name, glob)?;
    }
    let mut output = run_repo_rg(
        name,
        &repo,
        &paths,
        glob,
        &mode,
        &effective_query,
        "repo search",
    )?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(RuntimeError::Provider(format!(
            "tool {name} repo search failed: {}",
            provider_error_snippet(&String::from_utf8_lossy(&output.stderr))
        )));
    }
    if output.status.code() == Some(1) && should_smart_fallback(requested_mode, query) {
        mode = "smart".to_string();
        effective_query = repo_effective_search_query(&mode, query);
        output = run_repo_rg(
            name,
            &repo,
            &paths,
            glob,
            &mode,
            &effective_query,
            "repo search",
        )?;
        if !output.status.success() && output.status.code() != Some(1) {
            return Err(RuntimeError::Provider(format!(
                "tool {name} repo search failed: {}",
                provider_error_snippet(&String::from_utf8_lossy(&output.stderr))
            )));
        }
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
        "query_source": query_source,
        "effective_query": effective_query,
        "glob": glob,
        "mode": mode,
        "requested_mode": requested_mode,
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
                "query_source": query_source,
                "effective_query": effective_query,
                "glob": glob,
                "mode": mode,
                "requested_mode": requested_mode,
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
    let requested_mode = optional_string_input(name, input, "mode")?.unwrap_or("fixed");
    if !matches!(requested_mode, "fixed" | "regex" | "smart") {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.mode must be fixed, regex, or smart"
        )));
    }
    let mut mode = requested_mode.to_string();
    let mut effective_query = repo_effective_search_query(&mode, query);
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

    let glob = repo_glob_input(name, input)?;
    if let Some(glob) = glob {
        validate_git_pathspec(name, glob)?;
    }
    let mut output = run_repo_rg(
        name,
        &repo,
        &paths,
        glob,
        &mode,
        &effective_query,
        "repo context",
    )?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(RuntimeError::Provider(format!(
            "tool {name} repo context failed: {}",
            provider_error_snippet(&String::from_utf8_lossy(&output.stderr))
        )));
    }
    if output.status.code() == Some(1) && should_smart_fallback(requested_mode, query) {
        mode = "smart".to_string();
        effective_query = repo_effective_search_query(&mode, query);
        output = run_repo_rg(
            name,
            &repo,
            &paths,
            glob,
            &mode,
            &effective_query,
            "repo context",
        )?;
        if !output.status.success() && output.status.code() != Some(1) {
            return Err(RuntimeError::Provider(format!(
                "tool {name} repo context failed: {}",
                provider_error_snippet(&String::from_utf8_lossy(&output.stderr))
            )));
        }
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
        "effective_query": effective_query,
        "mode": mode,
        "requested_mode": requested_mode,
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
                "effective_query": effective_query,
                "mode": mode,
                "requested_mode": requested_mode,
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

fn repo_smart_search_terms(query: &str) -> Vec<String> {
    const STOP_WORDS: &[&str] = &[
        "about",
        "after",
        "again",
        "agent",
        "behavior",
        "change",
        "cleaner",
        "code",
        "does",
        "file",
        "from",
        "have",
        "implementation",
        "into",
        "make",
        "need",
        "preserve",
        "preserving",
        "refactor",
        "should",
        "task",
        "test",
        "that",
        "the",
        "this",
        "while",
        "with",
    ];
    let mut terms = Vec::new();
    for raw in query.split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
    {
        let term = raw.trim().to_ascii_lowercase();
        if term.len() < 3 || STOP_WORDS.contains(&term.as_str()) {
            continue;
        }
        if !terms.contains(&term) {
            terms.push(term);
        }
        if terms.len() >= 16 {
            break;
        }
    }
    terms
}

fn call_repo_symbols_tool(
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
    let mode = optional_string_input(name, input, "mode")?.unwrap_or("fixed");
    if !matches!(mode, "fixed" | "smart") {
        return Err(RuntimeError::Provider(format!(
            "tool {name} input.mode must be fixed or smart"
        )));
    }
    let query = raw_query.to_lowercase();
    let smart_terms = if mode == "smart" {
        repo_smart_search_terms(raw_query)
    } else {
        Vec::new()
    };
    let effective_query = if mode == "smart" {
        smart_terms.join("|")
    } else {
        query.clone()
    };
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
        if mode == "smart" {
            if !smart_terms.is_empty()
                && !repo_symbol_matches_any_term(&item, &symbol_name, text, &smart_terms)
            {
                continue;
            }
        } else if !query.is_empty()
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
        "query": raw_query,
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
    let empty_diagnostics = Vec::new();
    let diagnostics = match input.get("diagnostics") {
        Some(diagnostics) => diagnostics.as_array().ok_or_else(|| {
            RuntimeError::Provider(format!("tool {name} input.diagnostics must be an array"))
        })?,
        None => &empty_diagnostics,
    };
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
        let line = line
            .and_then(|line| usize::try_from(line).ok())
            .filter(|line| *line > 0)
            .unwrap_or(1);
        let line_defaulted = diagnostic
            .get("line")
            .and_then(Value::as_u64)
            .and_then(|line| usize::try_from(line).ok())
            .is_none_or(|line| line == 0);
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
                path_only_diagnostic_indexes: Vec::new(),
            });
        file.lines.push(line);
        file.diagnostic_indexes.push(index);
        if line_defaulted {
            file.path_only_diagnostic_indexes.push(index);
        }
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
            let path_only_diagnostic_indexes = diagnostic_indexes
                .iter()
                .copied()
                .filter(|index| file.path_only_diagnostic_indexes.contains(index))
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
                "path_only_diagnostic_indexes": path_only_diagnostic_indexes,
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
    path_only_diagnostic_indexes: Vec<usize>,
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
    parameters: &BTreeMap<String, CommandParameterRule>,
    options: CommandRunOptions,
) -> Result<Value, RuntimeError> {
    let command_name = input
        .get("command")
        .or_else(|| input.get("test_command"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            RuntimeError::Provider(format!("tool {name} input.command must be a string"))
        })?;
    let Some(command) = commands.get(command_name) else {
        return Err(RuntimeError::Provider(format!(
            "tool {name} command {command_name} is not configured"
        )));
    };
    let cwd = canonicalize_tool_path(name, "cwd", cwd)?;
    let (program, args) = command.split_first().ok_or_else(|| {
        RuntimeError::Provider(format!("tool {name} command {command_name} is empty"))
    })?;
    let rendered_args = render_command_args(name, input, args, parameters)?;
    let mut argv = Vec::with_capacity(command.len());
    argv.push(program.clone());
    argv.extend(rendered_args.iter().cloned());
    let mut child = Command::new(program)
        .args(&rendered_args)
        .current_dir(&cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| RuntimeError::Provider(format!("tool {name} command run: {error}")))?;
    let timeout = Duration::from_secs(options.timeout_seconds);
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
                let (log, truncated, bytes) = bytes_to_limited_text_with_direction(
                    &combined,
                    options.max_bytes,
                    options.truncation_direction,
                );
                let full_log_path = if truncated {
                    Some(save_command_full_log(name, command_name, &cwd, &combined)?)
                } else {
                    None
                };
                let truncation_hint = full_log_path.as_ref().map(|path| {
                    format!(
                        "The command output was truncated. Full command output saved to: {path}. Use file.search or file.read with a narrow range to inspect specific sections."
                    )
                });
                return Ok(json!({
                    "command": command_name,
                    "argv": argv,
                    "cwd": cwd.display().to_string(),
                    "status": status.code(),
                    "success": status.success(),
                    "log": log.clone(),
                    "diagnostics": diagnostics,
                    "bytes": bytes,
                    "truncated": truncated,
                    "full_log_path": full_log_path.clone(),
                    "truncation_hint": truncation_hint.clone(),
                    "artifacts": [{
                        "id": format!("command-run:{}:{}", cwd.display(), command_name),
                        "kind": "test_log",
                        "title": format!("command run: {command_name}"),
                        "uri": cwd.display().to_string(),
                        "content": log,
                        "metadata": {
                            "provider": "command_run",
                            "command": command_name,
                            "argv": argv,
                            "cwd": cwd.display().to_string(),
                            "status": status.code(),
                            "success": status.success(),
                            "diagnostics_count": diagnostics_count,
                            "bytes": bytes,
                            "truncated": truncated,
                            "full_log_path": full_log_path,
                            "truncation_hint": truncation_hint
                        }
                    }]
                }));
            }
            None if started_at.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RuntimeError::Provider(format!(
                    "tool {name} command {command_name} exceeded timeout_seconds={}",
                    options.timeout_seconds
                )));
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn save_command_full_log(
    name: &str,
    command_name: &str,
    cwd: &Path,
    combined: &[u8],
) -> Result<String, RuntimeError> {
    let output_dir = cwd.join(".air").join("tool-output");
    fs::create_dir_all(&output_dir).map_err(|error| {
        RuntimeError::Provider(format!(
            "tool {name} command {command_name} create output dir: {error}"
        ))
    })?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            RuntimeError::Provider(format!(
                "tool {name} command {command_name} timestamp: {error}"
            ))
        })?
        .as_millis();
    let filename = format!(
        "{}-{timestamp}.log",
        sanitize_command_output_filename(command_name)
    );
    let path = output_dir.join(filename);
    fs::write(&path, combined).map_err(|error| {
        RuntimeError::Provider(format!(
            "tool {name} command {command_name} write full log: {error}"
        ))
    })?;
    Ok(path.display().to_string())
}

fn sanitize_command_output_filename(command_name: &str) -> String {
    let sanitized = command_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "command".to_string()
    } else {
        trimmed.to_string()
    }
}

fn render_command_args(
    tool_name: &str,
    input: &Value,
    args: &[String],
    parameters: &BTreeMap<String, CommandParameterRule>,
) -> Result<Vec<String>, RuntimeError> {
    args.iter()
        .map(|arg| render_command_arg(tool_name, input, arg, parameters))
        .collect()
}

fn render_command_arg(
    tool_name: &str,
    input: &Value,
    arg: &str,
    parameters: &BTreeMap<String, CommandParameterRule>,
) -> Result<String, RuntimeError> {
    if !arg.contains("{{") {
        return Ok(arg.to_string());
    }

    let mut rendered = String::new();
    let mut rest = arg;
    while let Some(start) = rest.find("{{") {
        let (before, after_start) = rest.split_at(start);
        rendered.push_str(before);
        let after_start = &after_start[2..];
        let Some(end) = after_start.find("}}") else {
            return Err(RuntimeError::Provider(format!(
                "tool {tool_name} command template contains an unterminated parameter"
            )));
        };
        let parameter = after_start[..end].trim();
        if parameter.is_empty() {
            return Err(RuntimeError::Provider(format!(
                "tool {tool_name} command template contains an empty parameter"
            )));
        }
        let rule = parameters.get(parameter).ok_or_else(|| {
            RuntimeError::Provider(format!(
                "tool {tool_name} command template parameter {parameter} is not configured"
            ))
        })?;
        let value = required_command_parameter_input_string(tool_name, input, parameter)?;
        validate_command_parameter_value(tool_name, parameter, value, rule)?;
        rendered.push_str(value);
        rest = &after_start[end + 2..];
    }
    rendered.push_str(rest);
    Ok(rendered)
}

fn command_template_parameters(template: &str) -> Vec<String> {
    let mut parameters = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find("}}") else {
            break;
        };
        let parameter = after_start[..end].trim();
        if !parameter.is_empty() {
            parameters.push(parameter.to_string());
        }
        rest = &after_start[end + 2..];
    }
    parameters
}

fn required_command_parameter_input_string<'a>(
    tool_name: &str,
    input: &'a Value,
    field: &str,
) -> Result<&'a str, RuntimeError> {
    if let Some(value) = input.get(field) {
        return value.as_str().ok_or_else(|| {
            RuntimeError::Provider(format!("tool {tool_name} input.{field} must be a string"))
        });
    }
    for alias in ["args", "arguments"] {
        if let Some(value) = input.get(alias).and_then(|args| args.get(field)) {
            return value.as_str().ok_or_else(|| {
                RuntimeError::Provider(format!(
                    "tool {tool_name} input.{alias}.{field} must be a string"
                ))
            });
        }
    }
    Err(RuntimeError::Provider(format!(
        "tool {tool_name} input.{field} must be a string"
    )))
}

fn validate_command_parameter_value(
    tool_name: &str,
    parameter: &str,
    value: &str,
    rule: &CommandParameterRule,
) -> Result<(), RuntimeError> {
    if value.is_empty() {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.{parameter} must not be empty"
        )));
    }
    if let Some(max_chars) = rule.max_chars {
        let chars = value.chars().count();
        if chars > max_chars {
            return Err(RuntimeError::Provider(format!(
                "tool {tool_name} input.{parameter} must contain at most {max_chars} characters"
            )));
        }
    }
    if let Some(values) = &rule.values {
        if !values.iter().any(|allowed| allowed == value) {
            return Err(RuntimeError::Provider(format!(
                "tool {tool_name} input.{parameter} is not an allowed value"
            )));
        }
    }
    let valid = match rule.allow {
        CommandParameterAllow::Identifier => value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':')),
        CommandParameterAllow::Path => {
            validate_git_pathspec(tool_name, value).is_ok()
                && value.chars().all(|ch| {
                    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':')
                })
        }
        CommandParameterAllow::SafeArg => value.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(ch, '_' | '-' | '.' | '/' | ':' | '+' | '=' | '@')
        }),
    };
    if !valid {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.{parameter} contains characters not allowed by command parameter policy"
        )));
    }
    Ok(())
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

fn repo_search_query_input<'a>(
    tool_name: &str,
    input: &'a Value,
) -> Result<(&'a str, &'static str), RuntimeError> {
    if let Some(query) = input.get("query") {
        return query.as_str().map(|query| (query, "query")).ok_or_else(|| {
            RuntimeError::Provider(format!("tool {tool_name} input.query must be a string"))
        });
    }
    if let Some(pattern) = input.get("pattern") {
        return pattern
            .as_str()
            .map(|pattern| (pattern, "pattern"))
            .ok_or_else(|| {
                RuntimeError::Provider(format!("tool {tool_name} input.pattern must be a string"))
            });
    }
    Err(RuntimeError::Provider(format!(
        "tool {tool_name} input.query must be a string"
    )))
}

fn repo_glob_input<'a>(tool_name: &str, input: &'a Value) -> Result<Option<&'a str>, RuntimeError> {
    if let Some(glob) = input.get("glob") {
        return glob.as_str().map(Some).ok_or_else(|| {
            RuntimeError::Provider(format!("tool {tool_name} input.glob must be a string"))
        });
    }
    if let Some(glob) = input.get("file_glob") {
        return glob.as_str().map(Some).ok_or_else(|| {
            RuntimeError::Provider(format!("tool {tool_name} input.file_glob must be a string"))
        });
    }
    Ok(None)
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
    bytes_to_limited_text_with_direction(bytes, max_bytes, TruncationDirection::Head)
}

fn bytes_to_limited_text_with_direction(
    bytes: &[u8],
    max_bytes: usize,
    direction: TruncationDirection,
) -> (String, bool, usize) {
    let truncated = bytes.len() > max_bytes;
    let limited = if !truncated {
        bytes
    } else if matches!(direction, TruncationDirection::Tail) {
        &bytes[bytes.len() - max_bytes..]
    } else {
        &bytes[..max_bytes]
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

fn git_diff_paths(tool_name: &str, input: &Value) -> Result<(Vec<String>, bool), RuntimeError> {
    let mut paths = repo_tool_paths(tool_name, input)?;
    let mut has_filter = input.get("path").is_some() || input.get("paths").is_some();
    if let Some(raw_files) = input.get("files") {
        has_filter = true;
        let raw_files = raw_files.as_array().ok_or_else(|| {
            RuntimeError::Provider(format!("tool {tool_name} input.files must be an array"))
        })?;
        for (index, file) in raw_files.iter().enumerate() {
            let path = if let Some(path) = file.as_str() {
                path
            } else {
                file.as_object()
                    .and_then(|object| object.get("path"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        RuntimeError::Provider(format!(
                            "tool {tool_name} input.files[{index}] must be a string or object with path"
                        ))
                    })?
            };
            validate_git_pathspec(tool_name, path)?;
            paths.push(path.to_string());
        }
    }
    paths.sort();
    paths.dedup();
    Ok((paths, has_filter))
}

fn repo_tool_paths(tool_name: &str, input: &Value) -> Result<Vec<String>, RuntimeError> {
    let mut paths = Vec::new();
    if let Some(path) = input.get("path") {
        let path = path.as_str().ok_or_else(|| {
            RuntimeError::Provider(format!("tool {tool_name} input.path must be a string"))
        })?;
        if !path.trim().is_empty() {
            validate_git_pathspec(tool_name, path)?;
            paths.push(path.to_string());
        }
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
            if !path.trim().is_empty() {
                validate_git_pathspec(tool_name, path)?;
                paths.push(path.to_string());
            }
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

fn call_artifact_validate_tool(
    name: &str,
    input: &Value,
    fail_on_missing: bool,
    max_ids: usize,
) -> Result<Value, RuntimeError> {
    let mut registered_ids = BTreeSet::new();
    let mut registered_artifacts = Vec::new();
    if let Some(ids) = input.get("registered_ids") {
        collect_string_values(name, "registered_ids", ids, &mut registered_ids, max_ids)?;
    }
    if let Some(evidence) = input.get("evidence") {
        collect_artifact_ids(
            name,
            evidence,
            &mut registered_ids,
            &mut registered_artifacts,
            max_ids,
        )?;
    }

    let mut cited_ids = BTreeSet::new();
    if let Some(citations) = input.get("citations") {
        collect_citation_ids(name, citations, &mut cited_ids, max_ids)?;
    }
    if let Some(ids) = input.get("source_ids") {
        collect_string_values(name, "source_ids", ids, &mut cited_ids, max_ids)?;
    }
    if let Some(ids) = input.get("artifact_ids") {
        collect_string_values(name, "artifact_ids", ids, &mut cited_ids, max_ids)?;
    }

    let missing_ids = cited_ids
        .difference(&registered_ids)
        .cloned()
        .collect::<Vec<_>>();
    if fail_on_missing && !missing_ids.is_empty() {
        return Err(RuntimeError::Provider(format!(
            "tool {name} citations reference unregistered artifact ids: {}",
            missing_ids.join(", ")
        )));
    }

    let unused_registered_ids = registered_ids
        .difference(&cited_ids)
        .cloned()
        .collect::<Vec<_>>();
    let registered_ids = registered_ids.into_iter().collect::<Vec<_>>();
    let cited_ids = cited_ids.into_iter().collect::<Vec<_>>();
    let valid = missing_ids.is_empty();
    let content = format!(
        "valid: {valid}\nregistered_ids: {}\ncited_ids: {}\nmissing_ids: {}",
        registered_ids.len(),
        cited_ids.len(),
        missing_ids.len()
    );

    Ok(json!({
        "valid": valid,
        "registered_ids": registered_ids,
        "cited_ids": cited_ids,
        "missing_ids": missing_ids,
        "unused_registered_ids": unused_registered_ids,
        "registered_artifacts": registered_artifacts,
        "artifacts": [{
            "id": "artifact-validation:citations",
            "kind": "artifact_validation",
            "title": "citation artifact validation",
            "uri": "air://artifacts/validation/citations",
            "content": content,
            "metadata": {
                "provider": "artifact_validate",
                "valid": valid
            }
        }]
    }))
}

fn validate_test_command_allowlisted(
    name: &str,
    test_command: &str,
    allowed_test_commands: &[String],
) -> Result<(), RuntimeError> {
    if !allowed_test_commands
        .iter()
        .any(|allowed| allowed == test_command)
    {
        return Err(RuntimeError::Provider(format!(
            "tool {name} candidate test_command {test_command} is not allowlisted"
        )));
    }
    Ok(())
}

fn call_candidate_validate_tool(
    name: &str,
    input: &Value,
    base_dir: &Path,
    allowed_test_commands: &[String],
) -> Result<Value, RuntimeError> {
    let candidate = input.get("candidate").unwrap_or(input);
    let target_path = required_input_string(name, candidate, "target_path")?;
    validate_git_pathspec(name, target_path)?;
    let base_dir = canonicalize_tool_path(name, "base_dir", base_dir)?;
    let target_absolute =
        canonicalize_tool_path(name, "candidate.target_path", &base_dir.join(target_path))?;
    if !target_absolute.starts_with(&base_dir) || !target_absolute.is_file() {
        return Err(RuntimeError::Provider(format!(
            "tool {name} candidate target_path {target_path} is not a readable file under {}",
            base_dir.display()
        )));
    }

    let test_command = required_input_string(name, candidate, "test_command")?;
    validate_test_command_allowlisted(name, &test_command, allowed_test_commands)?;

    let mut related_files = Vec::new();
    if let Some(files) = candidate.get("related_files").and_then(Value::as_array) {
        for file in files {
            let Some(file) = file.as_str() else {
                return Err(RuntimeError::Provider(format!(
                    "tool {name} candidate.related_files entries must be strings"
                )));
            };
            validate_git_pathspec(name, file)?;
            let absolute =
                canonicalize_tool_path(name, "candidate.related_files", &base_dir.join(file))?;
            if !absolute.starts_with(&base_dir) || !absolute.is_file() {
                return Err(RuntimeError::Provider(format!(
                    "tool {name} candidate related file {file} is not a readable file under {}",
                    base_dir.display()
                )));
            }
            if file != target_path {
                related_files.push(file.to_string());
            }
        }
    }

    Ok(json!({
        "valid": true,
        "target_path": target_path,
        "related_files": related_files,
        "test_command": test_command,
        "artifacts": [{
            "id": format!("candidate-validation:{target_path}:{test_command}"),
            "kind": "candidate_validation",
            "title": "refactor candidate validation",
            "uri": format!("air://candidate/{target_path}"),
            "content": format!("valid: true\ntarget_path: {target_path}\ntest_command: {test_command}"),
            "metadata": {
                "provider": "candidate_validate",
                "valid": true,
                "target_path": target_path,
                "test_command": test_command
            }
        }]
    }))
}

fn collect_artifact_ids(
    tool_name: &str,
    value: &Value,
    ids: &mut BTreeSet<String>,
    artifacts: &mut Vec<Value>,
    max_ids: usize,
) -> Result<(), RuntimeError> {
    match value {
        Value::Object(object) => {
            if let Some(raw_artifacts) = object.get("artifacts") {
                let raw_artifacts = raw_artifacts.as_array().ok_or_else(|| {
                    RuntimeError::Provider(format!(
                        "tool {tool_name} evidence.artifacts must be an array"
                    ))
                })?;
                for artifact in raw_artifacts {
                    let Some(object) = artifact.as_object() else {
                        continue;
                    };
                    let Some(id) = object.get("id").and_then(Value::as_str) else {
                        continue;
                    };
                    insert_non_empty_id(tool_name, "evidence.artifacts.id", id, ids, max_ids)?;
                    artifacts.push(json!({
                        "id": id,
                        "kind": object.get("kind").cloned().unwrap_or(Value::Null),
                        "title": object.get("title").cloned().unwrap_or(Value::Null),
                        "uri": object.get("uri").cloned().unwrap_or(Value::Null)
                    }));
                }
            }
            for child in object.values() {
                collect_artifact_ids(tool_name, child, ids, artifacts, max_ids)?;
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_artifact_ids(tool_name, child, ids, artifacts, max_ids)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn collect_citation_ids(
    tool_name: &str,
    value: &Value,
    ids: &mut BTreeSet<String>,
    max_ids: usize,
) -> Result<(), RuntimeError> {
    collect_citation_ids_inner(tool_name, value, ids, max_ids, false)
}

fn collect_citation_ids_inner(
    tool_name: &str,
    value: &Value,
    ids: &mut BTreeSet<String>,
    max_ids: usize,
    collect_strings: bool,
) -> Result<(), RuntimeError> {
    match value {
        Value::String(id) => {
            if collect_strings {
                insert_non_empty_id(tool_name, "citations", id, ids, max_ids)
            } else {
                Ok(())
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_citation_ids_inner(tool_name, child, ids, max_ids, collect_strings)?;
            }
            Ok(())
        }
        Value::Object(object) => {
            for (key, child) in object {
                if is_citation_key(key) {
                    collect_string_values(tool_name, key, child, ids, max_ids)?;
                } else {
                    collect_citation_ids_inner(tool_name, child, ids, max_ids, false)?;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn is_citation_key(key: &str) -> bool {
    matches!(
        key,
        "source_id"
            | "source_ids"
            | "artifact_id"
            | "artifact_ids"
            | "citation"
            | "citations"
            | "source"
            | "sources"
    )
}

fn collect_string_values(
    tool_name: &str,
    field: &str,
    value: &Value,
    ids: &mut BTreeSet<String>,
    max_ids: usize,
) -> Result<(), RuntimeError> {
    match value {
        Value::String(id) => insert_non_empty_id(tool_name, field, id, ids, max_ids),
        Value::Array(values) => {
            for child in values {
                collect_string_values(tool_name, field, child, ids, max_ids)?;
            }
            Ok(())
        }
        Value::Object(object) => {
            for child in object.values() {
                collect_string_values(tool_name, field, child, ids, max_ids)?;
            }
            Ok(())
        }
        Value::Null => Ok(()),
        _ => Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.{field} must contain strings, arrays, or objects"
        ))),
    }
}

fn insert_non_empty_id(
    tool_name: &str,
    field: &str,
    id: &str,
    ids: &mut BTreeSet<String>,
    max_ids: usize,
) -> Result<(), RuntimeError> {
    let id = id.trim();
    if id.is_empty() {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.{field} entries must not be empty"
        )));
    }
    ids.insert(id.to_string());
    if ids.len() > max_ids {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} input.{field} must reference at most {max_ids} ids"
        )));
    }
    Ok(())
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

#[cfg(test)]
mod tests;
