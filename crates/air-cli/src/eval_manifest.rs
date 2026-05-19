use crate::code_artifact::{path_content_identity, sha256_hex};
use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const DEFAULT_EVAL_MANIFEST: &str = ".air/evals/manifest.json";

const DEFAULT_PROTECTED: &[&str] = &[
    ".air/evals/**",
    "skills/**/benches/**",
    "skills/**/regressions/**",
    "benches/**",
    "skills.lock",
];

const DEFAULT_INCLUDES: &[&str] = &["skills/code-agent/benches", "skills.lock"];

pub(crate) struct EvalManifestOptions {
    pub(crate) out: Option<PathBuf>,
    pub(crate) include: Vec<PathBuf>,
}

pub(crate) struct EvalCheckOptions {
    pub(crate) manifest: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
struct EvalManifest {
    schema: String,
    generated_at_unix: u64,
    files: Vec<EvalManifestFile>,
    protected: Vec<String>,
    holdout: EvalHoldout,
}

#[derive(Debug, Serialize, Deserialize)]
struct EvalManifestFile {
    path: String,
    sha256: String,
    bytes: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct EvalHoldout {
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct EvalCheckReport {
    pub(crate) schema: &'static str,
    pub(crate) manifest: String,
    pub(crate) passed: bool,
    pub(crate) checked_files: usize,
    pub(crate) missing: Vec<String>,
    pub(crate) changed: Vec<String>,
    pub(crate) protected_changed: Vec<String>,
}

pub(crate) fn write_eval_manifest(options: EvalManifestOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let out = absolutize(
        &cwd,
        options
            .out
            .unwrap_or_else(|| PathBuf::from(DEFAULT_EVAL_MANIFEST)),
    );
    let includes = if options.include.is_empty() {
        default_eval_includes(&cwd)?
    } else {
        options.include
    };
    let manifest = build_eval_manifest(&cwd, &includes, &out)?;
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(&out, serde_json::to_vec_pretty(&manifest)?)
        .with_context(|| format!("write {}", out.display()))?;
    println!("{}", serde_json::to_string_pretty(&manifest)?);
    Ok(())
}

pub(crate) fn check_eval_manifest_command(options: EvalCheckOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let manifest = absolutize(
        &cwd,
        options
            .manifest
            .unwrap_or_else(|| PathBuf::from(DEFAULT_EVAL_MANIFEST)),
    );
    let report = check_eval_manifest(&manifest)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !report.passed {
        anyhow::bail!("eval manifest check failed");
    }
    Ok(())
}

pub(crate) fn check_default_eval_manifest_if_present() -> Result<Option<EvalCheckReport>> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let manifest = cwd.join(DEFAULT_EVAL_MANIFEST);
    if !manifest.exists() {
        return Ok(None);
    }
    check_eval_manifest(&manifest).map(Some)
}

fn build_eval_manifest(cwd: &Path, includes: &[PathBuf], out: &Path) -> Result<EvalManifest> {
    let mut files = Vec::new();
    for include in includes {
        let path = absolutize(cwd, include.clone());
        collect_eval_files(cwd, &path, out, &mut files)?;
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    files.dedup_by(|left, right| left.path == right.path);
    let protected = default_protected_patterns(&files);
    Ok(EvalManifest {
        schema: "air.eval_manifest.v1".to_string(),
        generated_at_unix: unix_now(),
        files,
        protected,
        holdout: EvalHoldout::default(),
    })
}

fn default_eval_includes(cwd: &Path) -> Result<Vec<PathBuf>> {
    let mut includes = DEFAULT_INCLUDES
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    for path in git_tracked_files(cwd)? {
        if is_eval_integrity_source(&path) {
            includes.push(PathBuf::from(path));
        }
    }
    includes.sort();
    includes.dedup();
    Ok(includes)
}

fn default_protected_patterns(files: &[EvalManifestFile]) -> Vec<String> {
    let mut protected = DEFAULT_PROTECTED
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    for file in files {
        if is_eval_integrity_source(&file.path) {
            protected.push(file.path.clone());
        }
    }
    protected.sort();
    protected.dedup();
    protected
}

fn git_tracked_files(cwd: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["ls-files"])
        .current_dir(cwd)
        .output()
        .context("run git ls-files")?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.replace('\\', "/"))
        .collect())
}

fn is_eval_integrity_source(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    if normalized == "skills.lock"
        || normalized.contains("/benches/")
        || normalized.starts_with("benches/")
        || normalized.contains("/regressions/")
        || normalized.starts_with("regressions/")
    {
        return true;
    }
    let file_name = normalized.rsplit('/').next().unwrap_or(normalized.as_str());
    matches!(
        file_name,
        "audit.rs"
            | "bench.rs"
            | "code_artifact.rs"
            | "dream.rs"
            | "eval_manifest.rs"
            | "improve.rs"
            | "regression.rs"
            | "self_lab.rs"
    )
}

fn collect_eval_files(
    cwd: &Path,
    path: &Path,
    out: &Path,
    files: &mut Vec<EvalManifestFile>,
) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_file() {
        if same_path(path, out) {
            return Ok(());
        }
        if let Some(relative) = relative_path(cwd, path) {
            let identity = path_content_identity(path)?;
            let bytes = fs::metadata(path)
                .with_context(|| format!("metadata {}", path.display()))?
                .len();
            files.push(EvalManifestFile {
                path: relative,
                sha256: identity
                    .strip_prefix("sha256:")
                    .unwrap_or(&identity)
                    .to_string(),
                bytes,
            });
        }
        return Ok(());
    }
    for entry in fs::read_dir(path).with_context(|| format!("read {}", path.display()))? {
        let entry = entry.with_context(|| format!("read entry under {}", path.display()))?;
        let child = entry.path();
        let file_name = child
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if child.is_dir() {
            if matches!(
                file_name,
                ".git" | "node_modules" | ".venv" | ".cache" | "target"
            ) {
                continue;
            }
            collect_eval_files(cwd, &child, out, files)?;
        } else {
            collect_eval_files(cwd, &child, out, files)?;
        }
    }
    Ok(())
}

fn check_eval_manifest(path: &Path) -> Result<EvalCheckReport> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let manifest: EvalManifest =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    if manifest.schema != "air.eval_manifest.v1" {
        anyhow::bail!("unsupported eval manifest schema {}", manifest.schema);
    }
    let mut missing = Vec::new();
    let mut changed = Vec::new();
    for file in &manifest.files {
        let absolute = cwd.join(&file.path);
        if !absolute.exists() {
            missing.push(file.path.clone());
            continue;
        }
        let actual = sha256_hex(
            &fs::read(&absolute).with_context(|| format!("read {}", absolute.display()))?,
        );
        if actual != file.sha256 {
            changed.push(file.path.clone());
        }
    }
    let protected_changed = git_changed_protected_paths(&cwd, path, &manifest.protected)?;
    let passed = missing.is_empty() && changed.is_empty() && protected_changed.is_empty();
    Ok(EvalCheckReport {
        schema: "air.eval_manifest_check.v1",
        manifest: path.display().to_string(),
        passed,
        checked_files: manifest.files.len(),
        missing,
        changed,
        protected_changed,
    })
}

fn git_changed_protected_paths(
    cwd: &Path,
    manifest_path: &Path,
    protected: &[String],
) -> Result<Vec<String>> {
    let manifest_relative = relative_path(cwd, manifest_path);
    let protected = protected_globset(protected)?;
    let mut changed = Vec::new();
    for path in git_changed_paths(cwd)? {
        if manifest_relative.as_deref() == Some(path.as_str()) || protected.is_match(&path) {
            changed.push(path);
        }
    }
    changed.sort();
    changed.dedup();
    Ok(changed)
}

fn git_changed_paths(cwd: &Path) -> Result<Vec<String>> {
    let mut changed = Vec::new();
    for args in [
        &["diff", "--name-only", "HEAD", "--"][..],
        &["ls-files", "--others", "--exclude-standard"][..],
    ] {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .with_context(|| format!("run git {}", args.join(" ")))?;
        if !output.status.success() {
            continue;
        }
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let path = line.trim().replace('\\', "/");
            if !path.is_empty() {
                changed.push(path);
            }
        }
    }
    changed.sort();
    changed.dedup();
    Ok(changed)
}

fn protected_globset(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern).with_context(|| format!("parse glob {pattern}"))?);
    }
    builder.build().context("build eval protected globset")
}

fn relative_path(cwd: &Path, path: &Path) -> Option<String> {
    let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    path.strip_prefix(cwd)
        .ok()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
}

fn same_path(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    left == right
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn absolutize(cwd: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_protected_globs_match_eval_files() {
        let protected = default_protected_patterns(&[EvalManifestFile {
            path: "crates/air-cli/src/bench.rs".to_string(),
            sha256: "abc".to_string(),
            bytes: 3,
        }]);
        let set = protected_globset(&protected).unwrap();
        assert!(set.is_match("skills/code-agent/benches/rust-small/suite.json"));
        assert!(set.is_match("skills/code-agent/benches/regressions/imp-001.json"));
        assert!(set.is_match("crates/air-cli/src/bench.rs"));
        assert!(set.is_match(".air/evals/manifest.json"));
        assert!(!set.is_match("src/lib.rs"));
    }

    #[test]
    fn eval_integrity_source_detects_moved_runners() {
        assert!(is_eval_integrity_source("crates/foo/src/improve.rs"));
        assert!(is_eval_integrity_source("crates/foo/src/regression.rs"));
        assert!(is_eval_integrity_source("skills/x/benches/suite.json"));
        assert!(!is_eval_integrity_source("crates/foo/src/lib.rs"));
    }
}
