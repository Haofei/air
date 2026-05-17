use serde::Deserialize;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TruncationDirection {
    Head,
    Tail,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CommandRunOptions {
    pub(crate) timeout_seconds: u64,
    pub(crate) max_bytes: usize,
    pub(crate) truncation_direction: TruncationDirection,
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
    #[default]
    SafeArg,
}
