//! Cached LLM advisory pass.
//!
//! AIR's deterministic logic (audit verdicts, regression replay, eval integrity)
//! never delegates a decision to an LLM. Some surfaces, however, do benefit from
//! optional LLM color on top of deterministic output — audit-run diagnosis,
//! Dream synthesis hypotheses, failure-category classification of opaque
//! observations, task/skill routing reranks, candidate-judge tie-breaks.
//!
//! This crate centralizes the contract for those surfaces:
//!
//! * **Cached by input hash.** Same input → same output, even across process
//!   restarts and model-config swaps. Reruns are billable only on cache miss.
//! * **Validator closure rejects unsound output.** The caller passes a
//!   `validate(Value) -> Result<O>` that filters / clamps / cross-checks LLM
//!   output against deterministic evidence before it lands in the cache.
//! * **Two APIs**:
//!   * [`cached_llm_advisory`] returns `Result<O>` for callers that want to
//!     inspect the failure (e.g. write a sidecar error file).
//!   * [`cached_llm_advisory_or_fallback`] takes a deterministic-fallback
//!     closure and always returns `O`. Any LLM error invokes the fallback;
//!     the call site cannot accidentally `?`-propagate an LLM failure and
//!     break a command whose advisory layer was optional.
//! * **Kill switch.** `AIR_DISABLE_LLM_ADVISORY=1` makes every call return
//!   [`AdvisoryError::Disabled`]. The strong-contract API catches it and
//!   falls back; the result API surfaces it like any other error.
//! * **Cache schema verified on read.** Old caches with a different
//!   `air.llm_advisory_cache.vN` are rejected, not silently mis-decoded.
//!
//! The crate does NOT instantiate model providers. The caller passes in a
//! `&mut dyn ModelProvider`, so air-advisory has no opinion about HTTP,
//! fixtures, replay, or config file parsing.

#![deny(missing_docs)]

use air_runtime::ModelProvider;
use anyhow::{Context, Result};
use ring::digest;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

const LLM_ADVISORY_CACHE_SCHEMA: &str = air_schemas::LLM_ADVISORY_CACHE;
const DISABLE_ENV: &str = "AIR_DISABLE_LLM_ADVISORY";

/// `true` if the global LLM-advisory kill switch is set.
///
/// Callers can use this to short-circuit advisory pre-flight work (e.g.
/// constructing a model provider) before reaching [`cached_llm_advisory`].
pub fn is_disabled() -> bool {
    std::env::var_os(DISABLE_ENV).is_some()
}

/// Configuration for a single advisory call.
pub struct CachedLlmAdvisoryOptions<'a, I> {
    /// Directory under which the advisory cache lives.
    ///
    /// A subdirectory named [`purpose`] is created here; one JSON file per
    /// unique input hash sits inside it.
    ///
    /// [`purpose`]: Self::purpose
    pub cache_root: &'a Path,
    /// Optional path where a copy of the cache blob should be written for
    /// auditability (e.g., Dream writes its synthesis blob into the run output
    /// directory in addition to the global cache).
    pub out_file: Option<&'a Path>,
    /// Short identifier of the advisory site, used as the cache subdirectory
    /// name. Stable across releases. Examples: `dream-synthesis`,
    /// `audit-diagnosis`, `failure-classifier`, `task-router`, `skill-router`,
    /// `candidate-judge`.
    pub purpose: &'a str,
    /// Logical model alias passed to [`ModelProvider::call_model`]. This is
    /// the name a `model_config` maps to a real backend.
    pub model_alias: &'a str,
    /// Caller-supplied request payload. Hashed (in canonical JSON form) to
    /// form the cache key.
    pub request: &'a I,
    /// Unix seconds. Recorded on cache write; not part of the hash.
    pub generated_at_unix: u64,
}

/// Failure modes for [`cached_llm_advisory`].
///
/// `Disabled` is structurally distinct from other errors so that
/// [`cached_llm_advisory_or_fallback`] can tell whether the kill switch is
/// the reason for falling back.
#[derive(Debug, Error)]
pub enum AdvisoryError {
    /// `AIR_DISABLE_LLM_ADVISORY` was set; the call did not contact a model.
    #[error("LLM advisory is disabled by {DISABLE_ENV} for `{0}`")]
    Disabled(String),

    /// Generic failure (model error, validation failure, I/O failure on
    /// cache write, etc.).
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

impl AdvisoryError {
    /// `true` if this error is the kill-switch sentinel.
    pub fn is_disabled(&self) -> bool {
        matches!(self, AdvisoryError::Disabled(_))
    }
}

/// Result alias for the advisory API.
pub type AdvisoryResult<T> = std::result::Result<T, AdvisoryError>;

/// Call an LLM advisory, caching the output by input hash.
///
/// On cache hit the cached `result` field is decoded and returned without
/// re-running validation. Validation is treated as immutable per cache entry;
/// callers that tighten a validator must delete affected cache entries to
/// force re-validation.
///
/// On cache miss the model is invoked through `provider`, the response is
/// passed to `validate`, and the validated result is persisted. Validation
/// errors are NOT cached.
///
/// On `AIR_DISABLE_LLM_ADVISORY` this returns [`AdvisoryError::Disabled`]
/// before any cache or provider work.
pub fn cached_llm_advisory<I, O>(
    options: CachedLlmAdvisoryOptions<'_, I>,
    provider: &mut dyn ModelProvider,
    validate: impl FnOnce(Value) -> Result<O>,
) -> AdvisoryResult<O>
where
    I: Serialize,
    O: Serialize + DeserializeOwned,
{
    if is_disabled() {
        return Err(AdvisoryError::Disabled(options.purpose.to_string()));
    }
    let request_value = serde_json::to_value(options.request).map_err(into_other)?;
    let encoded = serde_json::to_vec(&request_value).map_err(into_other)?;
    let request_hash = format!("sha256:{}", sha256_hex(&encoded));
    let cache_dir = options.cache_root.join(options.purpose);
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("create {}", cache_dir.display()))
        .map_err(into_other)?;
    let cache_path = cache_dir.join(format!(
        "{}.json",
        request_hash.trim_start_matches("sha256:")
    ));

    if cache_path.exists() {
        copy_cache_to_out(&cache_path, options.out_file).map_err(into_other)?;
        let cached = read_json(&cache_path).map_err(into_other)?;
        if cached.get("schema").and_then(Value::as_str) != Some(LLM_ADVISORY_CACHE_SCHEMA) {
            return Err(AdvisoryError::Other(anyhow::anyhow!(
                "cached advisory {} has unsupported schema",
                cache_path.display()
            )));
        }
        // Cache hits intentionally trust the previously validated result.
        // Tightening a call-site validator requires deleting the cache entry.
        return serde_json::from_value(
            cached
                .get("result")
                .cloned()
                .unwrap_or_else(|| Value::Array(Vec::new())),
        )
        .with_context(|| format!("read cached advisory result {}", cache_path.display()))
        .map_err(into_other);
    }

    let output = provider
        .call_model(options.model_alias, &request_value)
        .map_err(|error| {
            AdvisoryError::Other(anyhow::anyhow!(
                "{} model call failed: {error}",
                options.model_alias
            ))
        })?;
    let result = validate(output.clone()).map_err(into_other)?;
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
    write_json_atomic(
        &cache_path,
        &serde_json::to_vec_pretty(&cache).map_err(into_other)?,
    )
    .map_err(into_other)?;
    copy_cache_to_out(&cache_path, options.out_file).map_err(into_other)?;
    serde_json::from_value(cache["result"].clone())
        .context("decode advisory result")
        .map_err(into_other)
}

/// Call an LLM advisory with a deterministic fallback.
///
/// This is the recommended entry point for advisory surfaces whose contract
/// is "LLM augments the deterministic value, never replaces correctness." The
/// `fallback` closure is invoked with the [`AdvisoryError`] when:
///
/// * The kill switch is set.
/// * The model provider returns an error (network failure, rate limit,
///   missing fixture, etc.).
/// * The validator rejects the LLM output.
/// * Cache I/O fails.
///
/// In every case the caller still receives an `O`, so wiring an advisory at
/// a call site cannot accidentally turn into a hard dependency on the model.
pub fn cached_llm_advisory_or_fallback<I, O>(
    options: CachedLlmAdvisoryOptions<'_, I>,
    provider: &mut dyn ModelProvider,
    validate: impl FnOnce(Value) -> Result<O>,
    fallback: impl FnOnce(AdvisoryError) -> O,
) -> O
where
    I: Serialize,
    O: Serialize + DeserializeOwned,
{
    match cached_llm_advisory(options, provider, validate) {
        Ok(result) => result,
        Err(error) => fallback(error),
    }
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

fn into_other(error: impl Into<anyhow::Error>) -> AdvisoryError {
    AdvisoryError::Other(error.into())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = digest::digest(&digest::SHA256, bytes);
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn write_json_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let tmp = temp_sibling(path, "tmp");
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .with_context(|| format!("write {}", tmp.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("write {}", tmp.display()))?;
    }
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} to {}", tmp.display(), path.display()))
}

fn temp_sibling(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("air-advisory");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    path.with_file_name(format!(
        ".{file_name}.{suffix}-{}-{nanos}",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use air_runtime::RuntimeError;
    use serde::Deserialize;
    use std::sync::Mutex;

    // `AIR_DISABLE_LLM_ADVISORY` is process-global; tests that touch it must
    // hold this mutex so other tests in the suite cannot observe a transient
    // value.
    static ENV_GUARD: Mutex<()> = Mutex::new(());

    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        match ENV_GUARD.lock() {
            Ok(guard) => guard,
            Err(poison) => poison.into_inner(),
        }
    }

    struct StaticProvider {
        response: Value,
        calls: usize,
    }

    impl ModelProvider for StaticProvider {
        fn call_model(&mut self, _name: &str, _input: &Value) -> Result<Value, RuntimeError> {
            self.calls += 1;
            Ok(self.response.clone())
        }
    }

    struct ErroringProvider;

    impl ModelProvider for ErroringProvider {
        fn call_model(&mut self, _name: &str, _input: &Value) -> Result<Value, RuntimeError> {
            Err(RuntimeError::Provider("provider down".to_string()))
        }
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    struct Decision {
        choice: String,
    }

    fn temp_root(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "air-advisory-test-{}-{nanos}-{label}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn second_call_with_same_input_hits_cache() {
        let _guard = env_guard();
        std::env::remove_var(DISABLE_ENV);
        let root = temp_root("cache");
        let mut provider = StaticProvider {
            response: json!({"choice": "code"}),
            calls: 0,
        };
        let request = json!({"task": "fix the parser"});
        let validate =
            |value: Value| -> Result<Decision> { Ok(serde_json::from_value(value).unwrap()) };
        let first = cached_llm_advisory::<_, Decision>(
            CachedLlmAdvisoryOptions {
                cache_root: &root,
                out_file: None,
                purpose: "test",
                model_alias: "model",
                request: &request,
                generated_at_unix: 1,
            },
            &mut provider,
            validate,
        )
        .unwrap();
        let second = cached_llm_advisory::<_, Decision>(
            CachedLlmAdvisoryOptions {
                cache_root: &root,
                out_file: None,
                purpose: "test",
                model_alias: "model",
                request: &request,
                generated_at_unix: 2,
            },
            &mut provider,
            |value| Ok(serde_json::from_value(value).unwrap()),
        )
        .unwrap();
        assert_eq!(
            first,
            Decision {
                choice: "code".to_string()
            }
        );
        assert_eq!(first, second);
        assert_eq!(provider.calls, 1, "second call must hit cache");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn disabled_env_makes_calls_return_sentinel() {
        let _guard = env_guard();
        let root = temp_root("disabled");
        std::env::set_var(DISABLE_ENV, "1");
        let mut provider = StaticProvider {
            response: json!({"choice": "code"}),
            calls: 0,
        };
        let result = cached_llm_advisory::<_, Decision>(
            CachedLlmAdvisoryOptions {
                cache_root: &root,
                out_file: None,
                purpose: "test",
                model_alias: "model",
                request: &json!({"task": "x"}),
                generated_at_unix: 1,
            },
            &mut provider,
            |value| Ok(serde_json::from_value(value).unwrap()),
        );
        std::env::remove_var(DISABLE_ENV);
        match result {
            Err(error) => {
                assert!(error.is_disabled(), "{error}");
            }
            Ok(_) => panic!("expected Disabled sentinel"),
        }
        assert_eq!(provider.calls, 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fallback_api_returns_deterministic_value_when_provider_errors() {
        let _guard = env_guard();
        std::env::remove_var(DISABLE_ENV);
        let root = temp_root("fallback");
        let mut provider = ErroringProvider;
        let value = cached_llm_advisory_or_fallback::<_, Decision>(
            CachedLlmAdvisoryOptions {
                cache_root: &root,
                out_file: None,
                purpose: "test",
                model_alias: "model",
                request: &json!({"task": "x"}),
                generated_at_unix: 1,
            },
            &mut provider,
            |value| Ok(serde_json::from_value(value).unwrap()),
            |_error| Decision {
                choice: "deterministic".to_string(),
            },
        );
        assert_eq!(value.choice, "deterministic");
        let _ = fs::remove_dir_all(root);
    }
}
