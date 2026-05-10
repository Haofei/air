use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

pub(crate) const CODE_AGENT_PACK_PATH: &str = "examples/code-agent/code-agent.air-pack.yaml";
const CODE_AGENT_PACK_YAML: &str =
    include_str!("../../../examples/code-agent/code-agent.air-pack.yaml");

#[derive(Debug, Deserialize)]
pub(crate) struct CodeAgentPack {
    pub(crate) recipes: Vec<CodeAgentPackRecipe>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CodeAgentPackRecipe {
    pub(crate) id: String,
    pub(crate) default_profile: PathBuf,
    #[serde(default)]
    pub(crate) intent: Option<String>,
}

pub(crate) fn default_code_agent_pack() -> Result<CodeAgentPack> {
    serde_yaml::from_str(CODE_AGENT_PACK_YAML).context("invalid code-agent AIR pack manifest")
}

pub(crate) fn default_profile_for_recipe(recipe: &str) -> Result<PathBuf> {
    Ok(default_recipe_for_id(recipe)?.default_profile)
}

pub(crate) fn default_recipe_for_id(recipe: &str) -> Result<CodeAgentPackRecipe> {
    let pack = default_code_agent_pack()?;
    let Some(pack_recipe) = pack
        .recipes
        .into_iter()
        .find(|pack_recipe| pack_recipe.id == recipe)
    else {
        bail!("code-agent pack is missing recipe {recipe}");
    };
    Ok(pack_recipe)
}
