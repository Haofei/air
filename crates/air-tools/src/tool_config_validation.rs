use super::{
    command_run::{command_template_parameters, validate_command_parameter_value},
    ToolConfig, ToolConfigFile,
};
use anyhow::{Context, Result};
use std::path::Path;

pub(super) fn validate_tool_config(config: &ToolConfigFile, path: &Path) -> Result<()> {
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
