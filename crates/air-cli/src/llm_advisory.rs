use crate::code_artifact::sha256_hex;
use crate::models::ModelProviderChoice;
use crate::ops::write_json_atomic;
use air_runtime::ModelProvider;
use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

const LLM_ADVISORY_CACHE_SCHEMA: &str = "air.llm_advisory_cache.v1";

pub(crate) struct CachedLlmAdvisoryOptions<'a, I> {
    pub(crate) cache_root: &'a Path,
    pub(crate) out_file: Option<&'a Path>,
    pub(crate) purpose: &'a str,
    pub(crate) model_alias: &'a str,
    pub(crate) model_config: &'a Path,
    pub(crate) request: &'a I,
    pub(crate) generated_at_unix: u64,
}

pub(crate) fn cached_llm_advisory<I, O>(
    options: CachedLlmAdvisoryOptions<'_, I>,
    validate: impl FnOnce(Value) -> Result<O>,
) -> Result<O>
where
    I: Serialize,
    O: Serialize + DeserializeOwned,
{
    if llm_advisory_disabled() {
        anyhow::bail!(
            "LLM advisory is disabled by AIR_DISABLE_LLM_ADVISORY for `{}`",
            options.purpose
        );
    }
    let request_value = serde_json::to_value(options.request)?;
    let encoded = serde_json::to_vec(&request_value)?;
    let request_hash = format!("sha256:{}", sha256_hex(&encoded));
    let cache_dir = options.cache_root.join(options.purpose);
    fs::create_dir_all(&cache_dir).with_context(|| format!("create {}", cache_dir.display()))?;
    let cache_path = cache_dir.join(format!(
        "{}.json",
        request_hash.trim_start_matches("sha256:")
    ));

    if cache_path.exists() {
        copy_cache_to_out(&cache_path, options.out_file)?;
        let cached = read_json(&cache_path)?;
        if cached.get("schema").and_then(Value::as_str) != Some(LLM_ADVISORY_CACHE_SCHEMA) {
            anyhow::bail!(
                "cached advisory {} has unsupported schema",
                cache_path.display()
            );
        }
        // Cache hits intentionally trust the previously validated result. Tightening a
        // call-site validator requires deleting the corresponding cache entry.
        return serde_json::from_value(
            cached
                .get("result")
                .cloned()
                .unwrap_or_else(|| Value::Array(Vec::new())),
        )
        .with_context(|| format!("read cached advisory result {}", cache_path.display()));
    }

    let mut provider = ModelProviderChoice::from_config_file_with_provider_io(
        options.model_config.to_path_buf(),
        false,
    )?;
    let output = provider
        .call_model(options.model_alias, &request_value)
        .map_err(|error| anyhow::anyhow!("{} model call failed: {error}", options.model_alias))?;
    let result = validate(output.clone())?;
    let cache = json!({
        "schema": LLM_ADVISORY_CACHE_SCHEMA,
        "purpose": options.purpose,
        "model": options.model_alias,
        "request_hash": request_hash,
        "generated_at_unix": options.generated_at_unix,
        "request": request_value,
        "output": output,
        "result": result,
    });
    write_json_atomic(&cache_path, &serde_json::to_vec_pretty(&cache)?)
        .with_context(|| format!("write {}", cache_path.display()))?;
    copy_cache_to_out(&cache_path, options.out_file)?;
    serde_json::from_value(cache["result"].clone()).context("decode advisory result")
}

pub(crate) fn llm_advisory_disabled() -> bool {
    std::env::var_os("AIR_DISABLE_LLM_ADVISORY").is_some()
}

fn copy_cache_to_out(cache_path: &Path, out_file: Option<&Path>) -> Result<()> {
    let Some(out_file) = out_file else {
        return Ok(());
    };
    if let Some(parent) = out_file.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::copy(cache_path, out_file)
        .with_context(|| format!("copy {} to {}", cache_path.display(), out_file.display()))?;
    Ok(())
}

fn read_json(path: &Path) -> Result<Value> {
    serde_json::from_slice(&fs::read(path).with_context(|| format!("read {}", path.display()))?)
        .with_context(|| format!("parse {}", path.display()))
}
