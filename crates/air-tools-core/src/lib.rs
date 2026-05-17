pub mod json_template;
pub mod text;
pub mod workspace_snapshot;

use serde::Deserialize;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TruncationDirection {
    Head,
    Tail,
}
