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

mod command_diagnostics;
mod context_tools;
mod edit_tools;
mod file_tools;
use context_tools::call_context_measure_tool;
use edit_tools::{call_file_edit_tool, FileEditOptions};
use file_tools::{
    call_file_read_many_tool, call_file_read_tool, call_file_search_tool, call_file_write_tool,
    read_snapshot, FileWriteOptions, ReadSnapshot,
};
mod helpdesk;
use helpdesk::helpdesk_docs;
mod provider;
pub use provider::{EchoTools, ToolProviderChoice};
mod repo_symbols;
use repo_symbols::{call_repo_symbols_tool, parse_symbol_declaration};
mod rust_lsp_tools;
use rust_lsp_tools::{call_lsp_diagnostics_tool, call_lsp_references_tool, RustAnalyzerSession};
mod command_run;
use command_run::{
    call_bash_tool, call_command_run_tool, command_template_parameters,
    validate_command_parameter_value,
};
mod todo_tools;
use todo_tools::call_todowrite_tool;

mod artifact_tools;
use artifact_tools::call_artifact_validate_tool;
mod bash_classify;
mod repo_discovery;
use repo_discovery::{call_repo_context_tool, call_repo_files_tool, call_repo_search_tool};
mod repo_reference_tools;
use repo_reference_tools::call_repo_references_tool;

const DEFAULT_CONTEXT_MAX_CHARS: usize = 200_000;
const DEFAULT_CONTEXT_THRESHOLD_PERCENT: u64 = 80;

#[derive(Debug, Deserialize)]
struct ToolConfigFile {
    #[serde(default)]
    workspace_dir: Option<PathBuf>,

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
    },
    FileEdit {
        #[serde(default)]
        capability: Option<String>,

        base_dir: PathBuf,

        #[serde(default)]
        max_bytes: Option<usize>,

        #[serde(default)]
        max_changed_lines: Option<usize>,
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
    RustAnalyzerReferences {
        #[serde(default)]
        capability: Option<String>,

        root_dir: PathBuf,

        #[serde(default)]
        command: Option<String>,

        #[serde(default)]
        max_results: Option<usize>,

        #[serde(default)]
        max_bytes: Option<usize>,
    },
    RustAnalyzerDiagnostics {
        #[serde(default)]
        capability: Option<String>,

        root_dir: PathBuf,

        #[serde(default)]
        command: Option<String>,

        #[serde(default)]
        max_diagnostics: Option<usize>,

        #[serde(default)]
        max_bytes: Option<usize>,
    },
    RustAnalyzer {
        #[serde(default)]
        capability: Option<String>,

        root_dir: PathBuf,

        #[serde(default)]
        command: Option<String>,

        #[serde(default)]
        max_results: Option<usize>,

        #[serde(default)]
        max_diagnostics: Option<usize>,

        #[serde(default)]
        max_bytes: Option<usize>,
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
    TodoWrite {
        #[serde(default)]
        capability: Option<String>,
    },
    Bash {
        #[serde(default)]
        capability: Option<String>,

        cwd: PathBuf,

        #[serde(default)]
        timeout_seconds: Option<u64>,

        #[serde(default)]
        max_bytes: Option<usize>,

        #[serde(default)]
        truncation_direction: Option<TruncationDirection>,
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
            | ToolConfig::RepoFiles { capability, .. }
            | ToolConfig::RepoSearch { capability, .. }
            | ToolConfig::RepoContext { capability, .. }
            | ToolConfig::RepoSymbols { capability, .. }
            | ToolConfig::RepoReferences { capability, .. }
            | ToolConfig::RustAnalyzerReferences { capability, .. }
            | ToolConfig::RustAnalyzerDiagnostics { capability, .. }
            | ToolConfig::RustAnalyzer { capability, .. }
            | ToolConfig::ContextMeasure { capability, .. }
            | ToolConfig::ArtifactValidate { capability, .. }
            | ToolConfig::TodoWrite { capability }
            | ToolConfig::Bash { capability, .. }
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

#[derive(Debug)]
pub struct ConfigTools {
    tools: BTreeMap<String, ToolConfig>,
    approvals: BTreeMap<String, ApprovalConfig>,
    config_dir: PathBuf,
    workspace_dir: PathBuf,
    read_snapshots: BTreeMap<PathBuf, ReadSnapshot>,
    read_observations: BTreeMap<PathBuf, Vec<ReadObservation>>,
    search_observations: BTreeMap<String, SearchObservation>,
    search_no_match_streak: u32,
    workspace_generation: u64,
    rust_analyzer_sessions: BTreeMap<String, RustAnalyzerSession>,
}

#[derive(Debug, Clone, Copy)]
struct ReadObservation {
    start_line: usize,
    end_line: usize,
    complete: bool,
    snapshot: ReadSnapshot,
}

#[derive(Debug, Clone)]
struct SearchObservation {
    path: String,
    pattern: String,
    match_count: u64,
    returned_match_count: u64,
}

impl Clone for ConfigTools {
    fn clone(&self) -> Self {
        Self {
            tools: self.tools.clone(),
            approvals: self.approvals.clone(),
            config_dir: self.config_dir.clone(),
            workspace_dir: self.workspace_dir.clone(),
            read_snapshots: self.read_snapshots.clone(),
            read_observations: self.read_observations.clone(),
            search_observations: self.search_observations.clone(),
            search_no_match_streak: self.search_no_match_streak,
            workspace_generation: self.workspace_generation,
            rust_analyzer_sessions: BTreeMap::new(),
        }
    }
}

impl ConfigTools {
    pub fn from_file(path: PathBuf) -> Result<Self> {
        let source = fs::read_to_string(&path)
            .with_context(|| format!("failed to read tool config {}", path.display()))?;
        let config: ToolConfigFile = serde_json::from_str(&source)
            .with_context(|| format!("failed to parse tool config JSON {}", path.display()))?;
        validate_tool_config(&config, &path)?;
        let config_dir = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let workspace_dir = if config.workspace_dir.is_some() {
            resolve_workspace_dir(config.workspace_dir.as_deref())?
        } else {
            config_dir.clone()
        };
        Ok(Self {
            tools: config.tools,
            approvals: config.approvals,
            config_dir,
            workspace_dir,
            read_snapshots: BTreeMap::new(),
            read_observations: BTreeMap::new(),
            search_observations: BTreeMap::new(),
            search_no_match_streak: 0,
            workspace_generation: 0,
            rust_analyzer_sessions: BTreeMap::new(),
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
            workspace_dir: PathBuf::from("."),
            read_snapshots: BTreeMap::new(),
            read_observations: BTreeMap::new(),
            search_observations: BTreeMap::new(),
            search_no_match_streak: 0,
            workspace_generation: 0,
            rust_analyzer_sessions: BTreeMap::new(),
        }
    }

    fn remember_read_snapshot(&mut self, path: &Path) -> Result<(), RuntimeError> {
        let path = fs::canonicalize(path).map_err(|error| {
            RuntimeError::Provider(format!("tool file snapshot canonicalize path: {error}"))
        })?;
        let snapshot = read_snapshot("tool", "path", &path)?;
        self.read_snapshots.insert(path, snapshot);
        Ok(())
    }

    fn note_workspace_may_have_changed(&mut self) {
        self.workspace_generation = self.workspace_generation.saturating_add(1);
        self.search_observations.clear();
        self.search_no_match_streak = 0;
    }

    fn remember_read_observation(
        &mut self,
        path: &Path,
        snapshot: ReadSnapshot,
        start_line: usize,
        end_line: usize,
        complete: bool,
    ) -> Result<(), RuntimeError> {
        let path = fs::canonicalize(path).map_err(|error| {
            RuntimeError::Provider(format!("tool file snapshot canonicalize path: {error}"))
        })?;
        self.read_snapshots.insert(path.clone(), snapshot);
        self.read_observations
            .entry(path)
            .or_default()
            .push(ReadObservation {
                start_line,
                end_line,
                complete,
                snapshot,
            });
        Ok(())
    }

    fn covered_read_observation(
        &self,
        path: &Path,
        snapshot: ReadSnapshot,
        start_line: usize,
        end_line: usize,
    ) -> Option<ReadObservation> {
        self.read_observations
            .get(path)?
            .iter()
            .copied()
            .find(|seen| {
                seen.complete
                    && seen.snapshot == snapshot
                    && seen.start_line == start_line
                    && seen.end_line == end_line
            })
    }

    fn rust_analyzer_session(
        &mut self,
        tool_name: &str,
        root_dir: &Path,
        command: &str,
    ) -> Result<&mut RustAnalyzerSession, RuntimeError> {
        let root = canonicalize_tool_path(tool_name, "root_dir", root_dir)?;
        let key = format!("{command}\n{}", root.display());
        if !self.rust_analyzer_sessions.contains_key(&key) {
            let session = RustAnalyzerSession::start(tool_name, &root, command)?;
            self.rust_analyzer_sessions.insert(key.clone(), session);
        }
        self.rust_analyzer_sessions.get_mut(&key).ok_or_else(|| {
            RuntimeError::Provider(format!(
                "tool {tool_name} failed to cache rust-analyzer session"
            ))
        })
    }

    fn annotate_search_progress(&mut self, mut output: Value) -> Value {
        if search_output_no_matches(&output) {
            self.search_no_match_streak = self.search_no_match_streak.saturating_add(1);
            insert_output_field(
                &mut output,
                "no_match_streak",
                Value::from(self.search_no_match_streak),
            );
            if self.search_no_match_streak >= 2 {
                append_output_search_hint(
                    &mut output,
                    "Several recent searches returned no matches; broaden the search strategy, inspect the project file list, or switch to a more likely source/test extension instead of repeating this direction.",
                );
            }
        } else if search_output_has_positive_results(&output) {
            self.search_no_match_streak = 0;
        }
        output
    }
}

const SEARCH_COUNT_FIELDS: &[&str] = &["match_count", "file_count", "returned_match_count"];
const SEARCH_ARRAY_FIELDS: &[&str] = &["matches", "files", "symbols", "references", "locations"];

fn search_output_no_matches(output: &Value) -> bool {
    output
        .get("no_matches")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn any_search_count_positive(output: &Value) -> bool {
    SEARCH_COUNT_FIELDS
        .iter()
        .any(|&field| output.get(field).and_then(Value::as_u64).unwrap_or(0) > 0)
}

fn any_search_array_nonempty(output: &Value) -> bool {
    SEARCH_ARRAY_FIELDS.iter().any(|&field| {
        output
            .get(field)
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
    })
}

fn search_output_has_positive_results(output: &Value) -> bool {
    !search_output_no_matches(output)
        && (any_search_count_positive(output) || any_search_array_nonempty(output))
}

fn insert_output_field(output: &mut Value, key: &str, value: Value) {
    if let Some(object) = output.as_object_mut() {
        object.insert(key.to_string(), value);
    }
}

fn append_output_search_hint(output: &mut Value, hint: &str) {
    let Some(object) = output.as_object_mut() else {
        return;
    };
    let existing = object
        .get("search_hint")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let next = if existing.trim().is_empty() {
        hint.to_string()
    } else if existing.contains(hint) {
        existing.to_string()
    } else {
        format!("{existing} {hint}")
    };
    object.insert("search_hint".to_string(), Value::String(next));
}

fn validate_tool_config(config: &ToolConfigFile, path: &Path) -> Result<()> {
    if config
        .workspace_dir
        .as_ref()
        .is_some_and(|workspace_dir| workspace_dir.as_os_str().is_empty())
    {
        anyhow::bail!(
            "tool config {} workspace_dir must not be empty",
            path.display()
        );
    }
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
                validate_max_bytes(path, name, *max_bytes)?;
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
                validate_non_empty_path(path, &format!("tools.{name}.base_dir"), base_dir)?;
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
                validate_non_empty_path(path, &format!("tools.{name}.base_dir"), base_dir)?;
                validate_max_bytes(path, name, *max_bytes)?;
            }
            ToolConfig::FileReadMany {
                base_dir,
                max_bytes,
                max_files,
                ..
            } => {
                validate_non_empty_path(path, &format!("tools.{name}.base_dir"), base_dir)?;
                validate_max_bytes(path, name, *max_bytes)?;
                validate_max_files(path, name, *max_files)?;
            }
            ToolConfig::FileSearch {
                base_dir,
                max_bytes,
                max_matches,
                max_context_lines,
                max_line_chars,
                ..
            } => {
                validate_non_empty_path(path, &format!("tools.{name}.base_dir"), base_dir)?;
                validate_max_bytes(path, name, *max_bytes)?;
                validate_max_matches(path, name, *max_matches)?;
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
                validate_non_empty_path(path, &format!("tools.{name}.base_dir"), base_dir)?;
                validate_max_bytes(path, name, *max_bytes)?;
            }
            ToolConfig::FileEdit {
                base_dir,
                max_bytes,
                max_changed_lines,
                ..
            } => {
                validate_non_empty_path(path, &format!("tools.{name}.base_dir"), base_dir)?;
                validate_max_bytes(path, name, *max_bytes)?;
                validate_positive_usize(
                    path,
                    &format!("tools.{name}.max_changed_lines"),
                    *max_changed_lines,
                )?;
            }
            ToolConfig::RepoFiles {
                repo_dir,
                max_files,
                ..
            } => {
                validate_non_empty_path(path, &format!("tools.{name}.repo_dir"), repo_dir)?;
                validate_max_files(path, name, *max_files)?;
            }
            ToolConfig::RepoSearch {
                repo_dir,
                max_matches,
                max_bytes,
                ..
            } => {
                validate_non_empty_path(path, &format!("tools.{name}.repo_dir"), repo_dir)?;
                validate_max_matches(path, name, *max_matches)?;
                validate_max_bytes(path, name, *max_bytes)?;
            }
            ToolConfig::RepoContext {
                repo_dir,
                max_matches,
                max_files,
                context_lines,
                max_bytes,
                ..
            } => {
                validate_non_empty_path(path, &format!("tools.{name}.repo_dir"), repo_dir)?;
                validate_max_matches(path, name, *max_matches)?;
                validate_max_files(path, name, *max_files)?;
                validate_positive_usize(
                    path,
                    &format!("tools.{name}.context_lines"),
                    *context_lines,
                )?;
                validate_max_bytes(path, name, *max_bytes)?;
            }
            ToolConfig::RepoSymbols {
                repo_dir,
                max_symbols,
                max_bytes,
                ..
            } => {
                validate_non_empty_path(path, &format!("tools.{name}.repo_dir"), repo_dir)?;
                validate_positive_usize(path, &format!("tools.{name}.max_symbols"), *max_symbols)?;
                validate_max_bytes(path, name, *max_bytes)?;
            }
            ToolConfig::RepoReferences {
                repo_dir,
                max_matches,
                max_files,
                context_lines,
                max_bytes,
                ..
            } => {
                validate_non_empty_path(path, &format!("tools.{name}.repo_dir"), repo_dir)?;
                validate_max_matches(path, name, *max_matches)?;
                validate_max_files(path, name, *max_files)?;
                validate_positive_usize(
                    path,
                    &format!("tools.{name}.context_lines"),
                    *context_lines,
                )?;
                validate_max_bytes(path, name, *max_bytes)?;
            }
            ToolConfig::RustAnalyzerReferences {
                root_dir,
                max_results,
                max_bytes,
                ..
            } => {
                validate_non_empty_path(path, &format!("tools.{name}.root_dir"), root_dir)?;
                validate_positive_usize(path, &format!("tools.{name}.max_results"), *max_results)?;
                validate_max_bytes(path, name, *max_bytes)?;
            }
            ToolConfig::RustAnalyzerDiagnostics {
                root_dir,
                max_diagnostics,
                max_bytes,
                ..
            } => {
                validate_non_empty_path(path, &format!("tools.{name}.root_dir"), root_dir)?;
                validate_positive_usize(
                    path,
                    &format!("tools.{name}.max_diagnostics"),
                    *max_diagnostics,
                )?;
                validate_max_bytes(path, name, *max_bytes)?;
            }
            ToolConfig::RustAnalyzer {
                root_dir,
                max_results,
                max_diagnostics,
                max_bytes,
                ..
            } => {
                validate_non_empty_path(path, &format!("tools.{name}.root_dir"), root_dir)?;
                validate_positive_usize(path, &format!("tools.{name}.max_results"), *max_results)?;
                validate_positive_usize(
                    path,
                    &format!("tools.{name}.max_diagnostics"),
                    *max_diagnostics,
                )?;
                validate_max_bytes(path, name, *max_bytes)?;
            }
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
            ToolConfig::TodoWrite { .. } => {}
            ToolConfig::Bash {
                cwd,
                timeout_seconds,
                max_bytes,
                truncation_direction: _,
                ..
            } => {
                validate_non_empty_path(path, &format!("tools.{name}.cwd"), cwd)?;
                validate_positive_u64(
                    path,
                    &format!("tools.{name}.timeout_seconds"),
                    *timeout_seconds,
                )?;
                validate_max_bytes(path, name, *max_bytes)?;
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
                validate_non_empty_path(path, &format!("tools.{name}.cwd"), cwd)?;
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
                validate_max_bytes(path, name, *max_bytes)?;
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

fn validate_max_bytes(path: &Path, name: &str, max_bytes: Option<usize>) -> Result<()> {
    validate_positive_usize(path, &format!("tools.{name}.max_bytes"), max_bytes)
}

fn validate_max_matches(path: &Path, name: &str, max_matches: Option<usize>) -> Result<()> {
    validate_positive_usize(path, &format!("tools.{name}.max_matches"), max_matches)
}

fn validate_max_files(path: &Path, name: &str, max_files: Option<usize>) -> Result<()> {
    validate_positive_usize(path, &format!("tools.{name}.max_files"), max_files)
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

fn validate_non_empty_path(path: &Path, field: &str, dir: &Path) -> Result<()> {
    if dir.as_os_str().is_empty() {
        anyhow::bail!("tool config {} {field} must not be empty", path.display());
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

fn observed_tool_key(generation: u64, name: &str, input: &Value) -> String {
    let input = serde_json::to_string(input).unwrap_or_else(|_| "<unserializable>".to_string());
    format!("{generation}\n{name}\n{input}")
}

fn repeated_read_output(
    previous_output: &Value,
    start_line: usize,
    end_line: usize,
    covered_by: ReadObservation,
) -> Value {
    json!({
        "path": previous_output.get("path").cloned().unwrap_or(Value::Null),
        "start_line": start_line,
        "end_line": end_line,
        "total_lines": previous_output.get("total_lines").cloned().unwrap_or(Value::Null),
        "no_new_information": true,
        "already_read": true,
        "covered_by": {
            "start_line": covered_by.start_line,
            "end_line": covered_by.end_line
        },
        "message": "This line range is already covered by a previous read and the file has not changed. Use the prior context, request a different line range, or edit based on the existing evidence.",
        "artifacts": [{
            "kind": "tool_notice",
            "title": "read skipped: already covered",
            "content": "No new file content returned because this unchanged line range was already read.",
            "metadata": {
                "provider": "file_read",
                "no_new_information": true,
                "already_read": true,
                "covered_start_line": covered_by.start_line,
                "covered_end_line": covered_by.end_line
            }
        }]
    })
}

fn repeated_search_output(previous: &SearchObservation) -> Value {
    json!({
        "path": previous.path.clone(),
        "pattern": previous.pattern.clone(),
        "match_count": previous.match_count,
        "returned_match_count": previous.returned_match_count,
        "no_new_information": true,
        "already_seen": true,
        "message": "This exact search was already run for the current workspace state. Use the previous matches, narrow the query, or edit based on the existing evidence.",
        "artifacts": [{
            "kind": "tool_notice",
            "title": "search skipped: already seen",
            "content": "No match list returned because this exact search was already run.",
            "metadata": {
                "provider": "file_search",
                "no_new_information": true,
                "already_seen": true,
                "match_count": previous.match_count,
                "returned_match_count": previous.returned_match_count
            }
        }]
    })
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
                        .map(|path| resolve_config_path(&self.workspace_dir, path)),
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
                    base_dir: &resolve_config_path(&self.workspace_dir, &base_dir),
                    screenshot_dir: screenshot_dir
                        .as_deref()
                        .map(|path| resolve_config_path(&self.workspace_dir, path)),
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
                let base_dir = resolve_config_path(&self.workspace_dir, &base_dir);
                let output =
                    call_file_read_tool(name, input, &base_dir, max_bytes.unwrap_or(256 * 1024))?;
                if let Some(path) = output.get("path").and_then(Value::as_str) {
                    let path = Path::new(path);
                    let snapshot = read_snapshot(name, "input.path", path)?;
                    let total_lines = output
                        .get("total_lines")
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as usize;
                    let start_line = output
                        .get("start_line")
                        .and_then(Value::as_u64)
                        .map(|line| line as usize)
                        .unwrap_or(1);
                    let end_line = output
                        .get("end_line")
                        .and_then(Value::as_u64)
                        .map(|line| line as usize)
                        .unwrap_or(total_lines);
                    if let Some(covered_by) =
                        self.covered_read_observation(path, snapshot, start_line, end_line)
                    {
                        self.read_snapshots.insert(path.to_path_buf(), snapshot);
                        return Ok(repeated_read_output(
                            &output, start_line, end_line, covered_by,
                        ));
                    }
                    let complete = !output
                        .get("truncated")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    self.remember_read_observation(path, snapshot, start_line, end_line, complete)?;
                }
                Ok(output)
            }
            ToolConfig::FileReadMany {
                capability: _,
                base_dir,
                max_bytes,
                max_files,
            } => {
                let base_dir = resolve_config_path(&self.workspace_dir, &base_dir);
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
                let base_dir = resolve_config_path(&self.workspace_dir, &base_dir);
                let observation_key = observed_tool_key(self.workspace_generation, name, input);
                if let Some(previous) = self.search_observations.get(&observation_key) {
                    return Ok(repeated_search_output(previous));
                }
                let output = call_file_search_tool(
                    name,
                    input,
                    &base_dir,
                    max_bytes.unwrap_or(64 * 1024),
                    max_matches.unwrap_or(100),
                    max_context_lines.unwrap_or(8),
                    max_line_chars.unwrap_or(2000),
                )?;
                self.search_observations.insert(
                    observation_key,
                    SearchObservation {
                        path: output
                            .get("path")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        pattern: output
                            .get("pattern")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        match_count: output
                            .get("match_count")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        returned_match_count: output
                            .get("returned_match_count")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                    },
                );
                Ok(self.annotate_search_progress(output))
            }
            ToolConfig::FileWrite {
                capability: _,
                base_dir,
                max_bytes,
                create_dirs,
                allow_overwrite,
            } => {
                let base_dir = resolve_config_path(&self.workspace_dir, &base_dir);
                let output = call_file_write_tool(
                    name,
                    input,
                    FileWriteOptions {
                        base_dir: &base_dir,
                        max_bytes: max_bytes.unwrap_or(256 * 1024),
                        create_dirs: create_dirs.unwrap_or(false),
                        allow_overwrite: allow_overwrite.unwrap_or(false),
                    },
                )?;
                if let Some(path) = output.get("path").and_then(Value::as_str) {
                    self.remember_read_snapshot(Path::new(path))?;
                }
                self.note_workspace_may_have_changed();
                Ok(output)
            }
            ToolConfig::FileEdit {
                capability: _,
                base_dir,
                max_bytes,
                max_changed_lines,
            } => {
                let output = call_file_edit_tool(
                    name,
                    input,
                    FileEditOptions {
                        base_dir: &resolve_config_path(&self.workspace_dir, &base_dir),
                        max_bytes: max_bytes.unwrap_or(256 * 1024),
                        max_changed_lines,
                    },
                )?;
                if let Some(path) = output.get("path").and_then(Value::as_str) {
                    self.remember_read_snapshot(Path::new(path))?;
                }
                self.note_workspace_may_have_changed();
                Ok(output)
            }
            ToolConfig::RepoFiles {
                capability: _,
                repo_dir,
                max_files,
            } => {
                let output = call_repo_files_tool(
                    name,
                    input,
                    &resolve_config_path(&self.workspace_dir, &repo_dir),
                    max_files.unwrap_or(200),
                )?;
                Ok(self.annotate_search_progress(output))
            }
            ToolConfig::RepoSearch {
                capability: _,
                repo_dir,
                max_matches,
                max_bytes,
            } => {
                let repo_dir = resolve_config_path(&self.workspace_dir, &repo_dir);
                let output = call_repo_search_tool(
                    name,
                    input,
                    &repo_dir,
                    max_matches.unwrap_or(80),
                    max_bytes.unwrap_or(256 * 1024),
                )?;
                Ok(self.annotate_search_progress(output))
            }
            ToolConfig::RepoContext {
                capability: _,
                repo_dir,
                max_matches,
                max_files,
                context_lines,
                max_bytes,
            } => {
                let repo_dir = resolve_config_path(&self.workspace_dir, &repo_dir);
                let output = call_repo_context_tool(
                    name,
                    input,
                    &repo_dir,
                    max_matches.unwrap_or(40),
                    max_files.unwrap_or(8),
                    context_lines.unwrap_or(6),
                    max_bytes.unwrap_or(256 * 1024),
                )?;
                Ok(self.annotate_search_progress(output))
            }
            ToolConfig::RepoSymbols {
                capability: _,
                repo_dir,
                max_symbols,
                max_bytes,
            } => call_repo_symbols_tool(
                name,
                input,
                &resolve_config_path(&self.workspace_dir, &repo_dir),
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
                let repo_dir = resolve_config_path(&self.workspace_dir, &repo_dir);
                call_repo_references_tool(
                    name,
                    input,
                    &repo_dir,
                    max_matches.unwrap_or(120),
                    max_files.unwrap_or(12),
                    context_lines.unwrap_or(4),
                    max_bytes.unwrap_or(256 * 1024),
                )
            }
            ToolConfig::RustAnalyzerReferences {
                capability: _,
                root_dir,
                command,
                max_results,
                max_bytes,
            } => {
                let root_dir = resolve_config_path(&self.workspace_dir, &root_dir);
                let command = command.as_deref().unwrap_or("rust-analyzer");
                let max_results = max_results.unwrap_or(120);
                let max_bytes = max_bytes.unwrap_or(256 * 1024);
                let session = self.rust_analyzer_session(name, &root_dir, command)?;
                call_lsp_references_tool(name, session, input, max_results, max_bytes)
            }
            ToolConfig::RustAnalyzerDiagnostics {
                capability: _,
                root_dir,
                command,
                max_diagnostics,
                max_bytes,
            } => call_lsp_diagnostics_tool(
                name,
                input,
                &resolve_config_path(&self.workspace_dir, &root_dir),
                command.as_deref().unwrap_or("rust-analyzer"),
                max_diagnostics.unwrap_or(80),
                max_bytes.unwrap_or(256 * 1024),
            ),
            ToolConfig::RustAnalyzer {
                capability: _,
                root_dir,
                command,
                max_results,
                max_diagnostics,
                max_bytes,
            } => {
                let root_dir = resolve_config_path(&self.workspace_dir, &root_dir);
                let command = command.as_deref().unwrap_or("rust-analyzer");
                let max_bytes = max_bytes.unwrap_or(256 * 1024);
                match lsp_command(name, input)? {
                    LspCommand::References => {
                        let session = self.rust_analyzer_session(name, &root_dir, command)?;
                        call_lsp_references_tool(
                            name,
                            session,
                            input,
                            max_results.unwrap_or(120),
                            max_bytes,
                        )
                    }
                    LspCommand::Diagnostics => call_lsp_diagnostics_tool(
                        name,
                        input,
                        &root_dir,
                        command,
                        max_diagnostics.unwrap_or(80),
                        max_bytes,
                    ),
                }
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
            ToolConfig::TodoWrite { capability: _ } => call_todowrite_tool(name, input),
            ToolConfig::Bash {
                capability: _,
                cwd,
                timeout_seconds,
                max_bytes,
                truncation_direction,
            } => {
                let output = call_bash_tool(
                    name,
                    input,
                    &resolve_config_path(&self.workspace_dir, &cwd),
                    CommandRunOptions {
                        timeout_seconds: timeout_seconds.unwrap_or(120),
                        max_bytes: max_bytes.unwrap_or(256 * 1024),
                        truncation_direction: truncation_direction
                            .unwrap_or(TruncationDirection::Tail),
                    },
                )?;
                self.note_workspace_may_have_changed();
                Ok(output)
            }
            ToolConfig::CommandRun {
                capability: _,
                cwd,
                commands,
                parameters,
                timeout_seconds,
                max_bytes,
                truncation_direction,
            } => {
                let output = call_command_run_tool(
                    name,
                    input,
                    &resolve_config_path(&self.workspace_dir, &cwd),
                    &commands,
                    &parameters,
                    CommandRunOptions {
                        timeout_seconds: timeout_seconds.unwrap_or(120),
                        max_bytes: max_bytes.unwrap_or(256 * 1024),
                        truncation_direction: truncation_direction
                            .unwrap_or(TruncationDirection::Head),
                    },
                )?;
                self.note_workspace_may_have_changed();
                Ok(output)
            }
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
            | ToolConfig::RepoFiles { capability, .. }
            | ToolConfig::RepoSearch { capability, .. }
            | ToolConfig::RepoContext { capability, .. }
            | ToolConfig::RepoSymbols { capability, .. }
            | ToolConfig::RepoReferences { capability, .. }
            | ToolConfig::RustAnalyzerReferences { capability, .. }
            | ToolConfig::RustAnalyzerDiagnostics { capability, .. }
            | ToolConfig::RustAnalyzer { capability, .. }
            | ToolConfig::ContextMeasure { capability, .. }
            | ToolConfig::ArtifactValidate { capability, .. }
            | ToolConfig::TodoWrite { capability }
            | ToolConfig::Bash { capability, .. }
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

fn run_playwright_subprocess(
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

    run_playwright_subprocess(name, "page audit", &script, &request, request_timeout)
}

fn insert_numeric_request_option(
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

enum LspCommand {
    References,
    Diagnostics,
}

fn lsp_command(name: &str, input: &Value) -> Result<LspCommand, RuntimeError> {
    let command = input
        .get("command")
        .or_else(|| input.get("method"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match command {
        Some("references") | Some("reference") | Some("refs") | Some("findReferences") => {
            Ok(LspCommand::References)
        }
        Some("diagnostics") | Some("diagnostic") | Some("check") => Ok(LspCommand::Diagnostics),
        Some(other) => Err(RuntimeError::Provider(format!(
            "tool {name} input.command must be references or diagnostics, got {other}"
        ))),
        None if input.get("symbol").is_some() || input.get("line").is_some() => {
            Ok(LspCommand::References)
        }
        None => Ok(LspCommand::Diagnostics),
    }
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

fn build_reference_entry(item: &Value, text: &str, definition_kind: Option<&str>) -> Value {
    json!({
        "path": item["path"],
        "line": item["line"],
        "column": item["column"],
        "text": text,
        "definition": definition_kind.is_some(),
        "kind": definition_kind.unwrap_or("reference")
    })
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

fn first_string_alias<'a>(
    tool_name: &str,
    input: &'a Value,
    aliases: &[&'static str],
) -> Result<Option<(&'a str, &'static str)>, RuntimeError> {
    for &alias in aliases {
        if let Some(value) = input.get(alias) {
            return value.as_str().map(|s| Some((s, alias))).ok_or_else(|| {
                RuntimeError::Provider(format!("tool {tool_name} input.{alias} must be a string"))
            });
        }
    }
    Ok(None)
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

pub(crate) fn optional_bounded_usize_input(
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

pub(crate) fn optional_threshold_percent_input(
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
        limited.len(),
    )
}

fn resolve_config_path(config_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        config_dir.join(path)
    }
}

fn resolve_workspace_dir(path: Option<&Path>) -> Result<PathBuf> {
    let Some(path) = path else {
        return Ok(PathBuf::from("."));
    };
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        let candidate = std::env::current_dir()?.join(path);
        if path == Path::new(".") {
            Ok(nearest_git_root(&candidate).unwrap_or(candidate))
        } else {
            Ok(candidate)
        }
    }
}

fn nearest_git_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
        .map(Path::to_path_buf)
}

fn repo_tool_paths(tool_name: &str, input: &Value) -> Result<Vec<String>, RuntimeError> {
    let mut paths = Vec::new();
    if let Some(path) = input.get("path") {
        let path = path.as_str().ok_or_else(|| {
            RuntimeError::Provider(format!("tool {tool_name} input.path must be a string"))
        })?;
        if !path.trim().is_empty() {
            validate_relative_path_filter(tool_name, path)?;
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
                validate_relative_path_filter(tool_name, path)?;
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

fn validate_relative_path_filter(tool_name: &str, path: &str) -> Result<(), RuntimeError> {
    let path = Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(RuntimeError::Provider(format!(
            "tool {tool_name} path filters must be relative paths inside repo_dir"
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

pub(crate) fn json_char_count(value: &Value) -> usize {
    serde_json::to_string(value)
        .unwrap_or_default()
        .chars()
        .count()
}

#[cfg(test)]
mod tests;
