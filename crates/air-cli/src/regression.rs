use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const DEFAULT_REGRESSION_DIR: &str = "skills/code-agent/benches/regressions";
const DEFAULT_IMPROVE_FROM: &str = "target/generated";

pub(crate) struct RegressionRunOptions {
    pub(crate) finding: Option<String>,
    pub(crate) file: Option<PathBuf>,
    pub(crate) all: bool,
    pub(crate) from: Vec<PathBuf>,
    pub(crate) out_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub(crate) struct RegressionRunConfig {
    pub(crate) finding: Option<String>,
    pub(crate) file: Option<PathBuf>,
    pub(crate) all: bool,
    pub(crate) from: Vec<PathBuf>,
    pub(crate) out_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RegressionRunReport {
    pub(crate) schema: &'static str,
    pub(crate) total: usize,
    pub(crate) passed: usize,
    pub(crate) failed: usize,
    pub(crate) results: Vec<RegressionResult>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RegressionResult {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) category: Option<String>,
    pub(crate) path: String,
    pub(crate) passed: bool,
    pub(crate) status: String,
    pub(crate) message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) command: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) report: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RegressionSpec {
    finding_id: Option<String>,
    id: Option<String>,
    kind: String,
    #[serde(default)]
    category: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FindingStatus {
    status: String,
    message: String,
}

pub(crate) fn run_regression_command(options: RegressionRunOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let out_dir = absolutize(
        &cwd,
        options
            .out_dir
            .unwrap_or_else(|| cwd.join(".air/regression/latest")),
    );
    let report = run_regression_suite(RegressionRunConfig {
        finding: options.finding,
        file: options.file,
        all: options.all,
        from: options
            .from
            .into_iter()
            .map(|path| absolutize(&cwd, path))
            .collect(),
        out_dir,
    })?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if report.failed > 0 {
        anyhow::bail!("{} of {} regression(s) failed", report.failed, report.total);
    }
    Ok(())
}

pub(crate) fn run_regression_suite(config: RegressionRunConfig) -> Result<RegressionRunReport> {
    fs::create_dir_all(&config.out_dir)
        .with_context(|| format!("create {}", config.out_dir.display()))?;
    let files = resolve_regression_files(&config)?;
    let mut results = Vec::new();
    for path in files {
        results.push(run_regression_file(&path, &config)?);
    }
    let passed = results.iter().filter(|result| result.passed).count();
    let total = results.len();
    Ok(RegressionRunReport {
        schema: "air.regression_run.v1",
        total,
        passed,
        failed: total.saturating_sub(passed),
        results,
    })
}

fn resolve_regression_files(config: &RegressionRunConfig) -> Result<Vec<PathBuf>> {
    if let Some(file) = &config.file {
        return Ok(vec![file.clone()]);
    }
    let dir = default_regression_dir();
    if let Some(finding) = &config.finding {
        let file = dir.join(format!("{}.json", finding.to_ascii_lowercase()));
        if file.exists() {
            return Ok(vec![file]);
        }
        anyhow::bail!("unknown regression `{finding}`");
    }
    if !config.all {
        anyhow::bail!("provide a finding id, --file, or --all");
    }
    let mut files = Vec::new();
    if dir.exists() {
        for entry in fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
            let path = entry
                .with_context(|| format!("read entry under {}", dir.display()))?
                .path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn run_regression_file(path: &Path, config: &RegressionRunConfig) -> Result<RegressionResult> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let spec: RegressionSpec =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    let id = spec
        .finding_id
        .clone()
        .or(spec.id.clone())
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("unknown")
                .to_string()
        });
    match spec.kind.as_str() {
        "code_run_verdict" => run_code_run_verdict_regression(path, &id, &spec, config),
        other => Ok(RegressionResult {
            id,
            kind: other.to_string(),
            category: spec.category,
            path: path.display().to_string(),
            passed: false,
            status: "unsupported".to_string(),
            message: format!("regression kind `{other}` is not executable yet"),
            command: Vec::new(),
            report: None,
        }),
    }
}

fn run_code_run_verdict_regression(
    path: &Path,
    id: &str,
    spec: &RegressionSpec,
    config: &RegressionRunConfig,
) -> Result<RegressionResult> {
    let executable = std::env::current_exe().context("resolve current air executable")?;
    let run_dir = config.out_dir.join(id.to_ascii_lowercase());
    let improve_dir = run_dir.join("improve");
    let mut args = vec![
        "improve".to_string(),
        "check".to_string(),
        id.to_string(),
        "--out-dir".to_string(),
        improve_dir.display().to_string(),
    ];
    let roots = if config.from.is_empty() {
        vec![PathBuf::from(DEFAULT_IMPROVE_FROM)]
    } else {
        config.from.clone()
    };
    for root in roots {
        args.push("--from".to_string());
        args.push(root.display().to_string());
    }
    let status = Command::new(&executable)
        .args(&args)
        .status()
        .with_context(|| format!("run {}", executable.display()))?;
    let status_path = improve_dir
        .join("status")
        .join(format!("{}.json", id.to_ascii_lowercase()));
    let (passed, status_label, message) = if status_path.exists() {
        let finding_status: FindingStatus = serde_json::from_str(
            &fs::read_to_string(&status_path)
                .with_context(|| format!("read {}", status_path.display()))?,
        )
        .with_context(|| format!("parse {}", status_path.display()))?;
        (
            status.success() && finding_status.status == "stale",
            finding_status.status,
            finding_status.message,
        )
    } else {
        (
            false,
            "missing_status".to_string(),
            "improve check did not write a finding status".to_string(),
        )
    };
    let mut command = vec![executable.display().to_string()];
    command.extend(args);
    Ok(RegressionResult {
        id: id.to_string(),
        kind: spec.kind.clone(),
        category: spec.category.clone(),
        path: path.display().to_string(),
        passed,
        status: status_label,
        message,
        command,
        report: Some(
            improve_dir
                .join("checks")
                .join(format!("{}-check.md", id.to_ascii_lowercase()))
                .display()
                .to_string(),
        ),
    })
}

fn absolutize(cwd: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

fn default_regression_dir() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for base in [
        cwd.as_path(),
        cwd.parent().unwrap_or(cwd.as_path()),
        cwd.parent().and_then(Path::parent).unwrap_or(cwd.as_path()),
    ] {
        let candidate = base.join(DEFAULT_REGRESSION_DIR);
        if candidate.exists() {
            return candidate;
        }
    }
    PathBuf::from(DEFAULT_REGRESSION_DIR)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_regression_by_finding_id() {
        let config = RegressionRunConfig {
            finding: Some("IMP-003".to_string()),
            file: None,
            all: false,
            from: Vec::new(),
            out_dir: PathBuf::from(".air/regression/test"),
        };
        let files = resolve_regression_files(&config).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("skills/code-agent/benches/regressions/imp-003.json"));
    }
}
