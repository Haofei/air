use super::ToolConfig;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub(super) struct ToolConfigFile {
    #[serde(default)]
    pub(super) workspace_dir: Option<PathBuf>,

    #[serde(default)]
    pub(super) tools: BTreeMap<String, ToolConfig>,

    #[serde(default)]
    pub(super) approvals: BTreeMap<String, ApprovalConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ApprovalConfig {
    pub(super) approved: bool,

    #[serde(default)]
    pub(super) approver: Option<String>,

    #[serde(default)]
    pub(super) reason: Option<String>,

    #[serde(default)]
    pub(super) metadata: Option<Value>,
}
