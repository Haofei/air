use std::collections::BTreeSet;

use air_tools_core::TruncationDirection;
use serde::Deserialize;

const DEFAULT_WORKSPACE_SNAPSHOT_IGNORE: &[&str] = &[
    ".air",
    ".air/**",
    ".git",
    ".git/**",
    ".cache",
    ".cache/**",
    ".next",
    ".next/**",
    ".venv",
    ".venv/**",
    "__pycache__",
    "__pycache__/**",
    "build",
    "build/**",
    "coverage",
    "coverage/**",
    "dist",
    "dist/**",
    "node_modules",
    "node_modules/**",
    "target",
    "target/**",
    "venv",
    "venv/**",
];

#[derive(Debug, Clone)]
pub(crate) struct CommandRunOptions {
    pub(crate) timeout_seconds: u64,
    pub(crate) max_bytes: usize,
    pub(crate) truncation_direction: TruncationDirection,
    pub(crate) workspace_snapshot_ignore: Vec<String>,
}

pub(crate) fn workspace_snapshot_ignore_patterns(configured: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    DEFAULT_WORKSPACE_SNAPSHOT_IGNORE
        .iter()
        .map(|pattern| (*pattern).to_string())
        .chain(configured.iter().cloned())
        .filter(|pattern| seen.insert(pattern.clone()))
        .collect()
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CommandParameterRule {
    #[serde(default)]
    pub(crate) values: Option<Vec<String>>,

    #[serde(default)]
    pub(crate) max_chars: Option<usize>,

    #[serde(default)]
    pub(crate) allow: CommandParameterAllow,
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CommandParameterAllow {
    Identifier,
    Path,
    Text,
    #[default]
    SafeArg,
}
