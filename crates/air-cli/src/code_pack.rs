use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

const CODE_AGENT_PACK_YAML: &str =
    include_str!("../../../examples/code-agent/code-agent.air-pack.yaml");

#[derive(Debug, Deserialize)]
pub(crate) struct CodeAgentPack {
    pub(crate) recipes: Vec<CodeAgentPackRecipe>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CodeAgentPackRecipe {
    pub(crate) id: String,
    pub(crate) default_profile: PathBuf,
}

pub(crate) fn default_code_agent_pack() -> Result<CodeAgentPack> {
    serde_yaml::from_str(CODE_AGENT_PACK_YAML).context("invalid code-agent AIR pack manifest")
}

pub(crate) fn default_profile_for_recipe(recipe: &str) -> Result<PathBuf> {
    let pack = default_code_agent_pack()?;
    let Some(pack_recipe) = pack
        .recipes
        .into_iter()
        .find(|pack_recipe| pack_recipe.id == recipe)
    else {
        bail!("code-agent pack is missing recipe {recipe}");
    };
    Ok(pack_recipe.default_profile)
}
