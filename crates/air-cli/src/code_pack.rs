use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const CODE_AGENT_PACK_PATH: &str = "examples/code-agent/code-agent.air-pack.yaml";
const CODE_AGENT_PACK_YAML: &str =
    include_str!("../../../examples/code-agent/code-agent.air-pack.yaml");

#[derive(Clone, Debug, Deserialize)]
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

#[derive(Clone, Debug)]
pub(crate) struct CodeAgentPackContext {
    pub(crate) path: PathBuf,
    pub(crate) pack: CodeAgentPack,
}

pub(crate) fn load_code_agent_pack(path: Option<PathBuf>) -> Result<CodeAgentPackContext> {
    match path {
        Some(path) => {
            let pack = read_code_agent_pack(&path)?;
            Ok(CodeAgentPackContext { path, pack })
        }
        None => Ok(CodeAgentPackContext {
            path: PathBuf::from(CODE_AGENT_PACK_PATH),
            pack: default_code_agent_pack()?,
        }),
    }
}

pub(crate) fn default_code_agent_pack() -> Result<CodeAgentPack> {
    serde_yaml::from_str(CODE_AGENT_PACK_YAML).context("invalid code-agent AIR pack manifest")
}

pub(crate) fn read_code_agent_pack(path: &Path) -> Result<CodeAgentPack> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read code-agent pack {}", path.display()))?;
    serde_yaml::from_str(&text)
        .with_context(|| format!("invalid code-agent AIR pack manifest {}", path.display()))
}

impl CodeAgentPackContext {
    pub(crate) fn default_profile_for_recipe(&self, recipe: &str) -> Result<PathBuf> {
        Ok(self.recipe_for_id(recipe)?.default_profile)
    }

    pub(crate) fn recipe_for_id(&self, recipe: &str) -> Result<CodeAgentPackRecipe> {
        let Some(pack_recipe) = self
            .pack
            .recipes
            .iter()
            .find(|pack_recipe| pack_recipe.id == recipe)
            .cloned()
        else {
            bail!(
                "code-agent pack {} is missing recipe {recipe}",
                self.path.display()
            );
        };
        Ok(pack_recipe)
    }
}
