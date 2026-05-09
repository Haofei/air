use air_core::AirModule;
use std::fs;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("unsupported AIR file extension for {0}; expected .yaml, .yml, or .json")]
    UnsupportedExtension(String),

    #[error("failed to parse YAML AIR module: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("failed to parse JSON AIR module: {0}")]
    Json(#[from] serde_json::Error),
}

pub fn parse_air_file(path: impl AsRef<Path>) -> Result<AirModule, ParseError> {
    let path = path.as_ref();
    let source = fs::read_to_string(path).map_err(|source| ParseError::Read {
        path: path.display().to_string(),
        source,
    })?;

    match path.extension().and_then(|extension| extension.to_str()) {
        Some("yaml") | Some("yml") => Ok(serde_yaml::from_str(&source)?),
        Some("json") => Ok(serde_json::from_str(&source)?),
        _ => Err(ParseError::UnsupportedExtension(path.display().to_string())),
    }
}
