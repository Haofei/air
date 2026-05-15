#[cfg(test)]
use crate::code_pack::load_code_agent_pack;
use crate::code_pack::CodeAgentPackContext;
use anyhow::Result;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

pub(crate) struct CodeInputOptions {
    pub(crate) task: String,
}

#[cfg(test)]
pub(crate) fn build_input(options: CodeInputOptions) -> Result<Map<String, Value>> {
    let pack = load_code_agent_pack(None)?;
    build_input_with_pack(&pack, options)
}

pub(crate) fn build_input_with_pack(
    _pack: &CodeAgentPackContext,
    options: CodeInputOptions,
) -> Result<Map<String, Value>> {
    let mut input = Map::new();
    input.insert("task".to_string(), Value::String(options.task));
    Ok(input)
}

pub(crate) fn default_profile(pack: &CodeAgentPackContext) -> Result<PathBuf> {
    Ok(pack.default_profile())
}

pub(crate) fn path_ref_to_input_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_input_is_just_the_task() {
        let input = build_input(CodeInputOptions {
            task: "split context tool".to_string(),
        })
        .unwrap();

        assert_eq!(input.len(), 1);
        assert_eq!(input["task"], "split context tool");
    }
}
