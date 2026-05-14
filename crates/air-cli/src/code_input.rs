#[cfg(test)]
use crate::code_pack::load_code_agent_pack;
use crate::code_pack::CodeAgentPackContext;
use anyhow::Result;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CodeRecipe {
    /// Unified code edit loop: model-selected read/search/edit/test tools under AIR policy.
    Edit,
}

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
    input.insert(
        "acceptance_assertions".to_string(),
        Value::Array(Vec::new()),
    );
    Ok(input)
}

pub(crate) fn default_profile(pack: &CodeAgentPackContext, recipe: CodeRecipe) -> Result<PathBuf> {
    pack.default_profile_for_recipe(recipe_name(recipe))
}

pub(crate) fn recipe_name(recipe: CodeRecipe) -> &'static str {
    match recipe {
        CodeRecipe::Edit => "edit",
    }
}

pub(crate) fn path_ref_to_input_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn edit_input_does_not_guess_split_acceptance_assertions() {
        let input = build_input(CodeInputOptions {
            task: "split context tool".to_string(),
        })
        .unwrap();

        assert_eq!(
            input.get("acceptance_assertions").unwrap(),
            &json!([]),
            "AIR should not invent structural postconditions from a natural-language edit task"
        );
    }
}
