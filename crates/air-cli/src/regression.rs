use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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

struct RegressionStatus {
    passed: bool,
    status: String,
    message: String,
    matching_finding: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RegressionSpec {
    finding_id: Option<String>,
    id: Option<String>,
    kind: String,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    fixture_hint: Option<Value>,
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

    let mut command = vec![executable.display().to_string()];
    command.extend(args.clone());
    let mut report = improve_dir.join("report.md").display().to_string();
    let regression_status = if status.success() {
        regression_observation_status(&improve_dir, spec)?
    } else {
        RegressionStatus {
            passed: false,
            status: "improve_failed".to_string(),
            message: "improve mining failed before regression status could be derived".to_string(),
            matching_finding: None,
        }
    };
    let regression_status = if regression_status.status == "active_failure" {
        if let Some(finding_id) = regression_status.matching_finding.as_deref() {
            let check = run_matching_finding_check(&executable, &improve_dir, finding_id, config)?;
            command = check.command;
            report = check.report;
            check.status
        } else {
            regression_status
        }
    } else {
        regression_status
    };
    Ok(RegressionResult {
        id: id.to_string(),
        kind: spec.kind.clone(),
        category: spec.category.clone(),
        path: path.display().to_string(),
        passed: regression_status.passed,
        status: regression_status.status,
        message: regression_status.message,
        command,
        report: Some(report),
    })
}

fn regression_observation_status(
    improve_dir: &Path,
    spec: &RegressionSpec,
) -> Result<RegressionStatus> {
    let observations_path = improve_dir.join("observations.json");
    let value: Value = serde_json::from_str(
        &fs::read_to_string(&observations_path)
            .with_context(|| format!("read {}", observations_path.display()))?,
    )
    .with_context(|| format!("parse {}", observations_path.display()))?;
    let expected_category = spec
        .fixture_hint
        .as_ref()
        .and_then(|hint| hint.pointer("/expected/category"))
        .and_then(Value::as_str)
        .or(spec.category.as_deref())
        .map(normalize_category);
    let expected_final_success = spec
        .fixture_hint
        .as_ref()
        .and_then(|hint| hint.pointer("/expected/final_success"))
        .and_then(Value::as_bool);
    let matches = value
        .get("observations")
        .and_then(Value::as_array)
        .map(|observations| {
            observations
                .iter()
                .filter(|observation| {
                    observation_matches_expected(
                        observation,
                        expected_category.as_deref(),
                        expected_final_success,
                    )
                })
                .count()
        })
        .unwrap_or(0);
    if matches == 0 {
        return Ok(RegressionStatus {
            passed: true,
            status: "stale".to_string(),
            message: "No current observation matches the promoted regression failure signature."
                .to_string(),
            matching_finding: None,
        });
    }
    Ok(RegressionStatus {
        passed: false,
        status: "active_failure".to_string(),
        message: format!(
            "{matches} current observation(s) still match the promoted regression failure signature."
        ),
        matching_finding: matching_finding_id(improve_dir, expected_category.as_deref())?,
    })
}

struct MatchingCheck {
    status: RegressionStatus,
    command: Vec<String>,
    report: String,
}

fn run_matching_finding_check(
    executable: &Path,
    improve_dir: &Path,
    finding_id: &str,
    config: &RegressionRunConfig,
) -> Result<MatchingCheck> {
    let check_dir = improve_dir.join("check");
    let mut args = vec![
        "improve".to_string(),
        "check".to_string(),
        finding_id.to_string(),
        "--out-dir".to_string(),
        check_dir.display().to_string(),
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
    let status = Command::new(executable)
        .args(&args)
        .status()
        .with_context(|| format!("run {}", executable.display()))?;
    let status_path = check_dir
        .join("status")
        .join(format!("{}.json", finding_id.to_ascii_lowercase()));
    let regression_status = if status_path.exists() {
        let finding_status: FindingStatus = serde_json::from_str(
            &fs::read_to_string(&status_path)
                .with_context(|| format!("read {}", status_path.display()))?,
        )
        .with_context(|| format!("parse {}", status_path.display()))?;
        let resolved = status.success() && finding_status.status == "stale";
        RegressionStatus {
            passed: resolved,
            status: if resolved {
                "resolved".to_string()
            } else {
                finding_status.status
            },
            message: if resolved {
                "The smallest known benchmark reproduction now passes under this candidate."
                    .to_string()
            } else {
                finding_status.message
            },
            matching_finding: Some(finding_id.to_string()),
        }
    } else {
        RegressionStatus {
            passed: false,
            status: "missing_status".to_string(),
            message: "improve check did not write a finding status".to_string(),
            matching_finding: Some(finding_id.to_string()),
        }
    };
    let mut command = vec![executable.display().to_string()];
    command.extend(args);
    Ok(MatchingCheck {
        status: regression_status,
        command,
        report: check_dir
            .join("checks")
            .join(format!("{}-check.md", finding_id.to_ascii_lowercase()))
            .display()
            .to_string(),
    })
}

fn matching_finding_id(
    improve_dir: &Path,
    expected_category: Option<&str>,
) -> Result<Option<String>> {
    let findings_path = improve_dir.join("findings.json");
    let value: Value = serde_json::from_str(
        &fs::read_to_string(&findings_path)
            .with_context(|| format!("read {}", findings_path.display()))?,
    )
    .with_context(|| format!("parse {}", findings_path.display()))?;
    Ok(value
        .get("findings")
        .and_then(Value::as_array)
        .and_then(|findings| {
            findings.iter().find_map(|finding| {
                let category = finding
                    .get("category")
                    .and_then(Value::as_str)
                    .map(normalize_category);
                if expected_category.is_some() && category.as_deref() != expected_category {
                    return None;
                }
                finding
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
        }))
}

fn observation_matches_expected(
    observation: &Value,
    expected_category: Option<&str>,
    expected_final_success: Option<bool>,
) -> bool {
    if let Some(category) = expected_category {
        let actual = observation
            .get("category")
            .and_then(Value::as_str)
            .map(normalize_category);
        if actual.as_deref() != Some(category) {
            return false;
        }
    }
    if let Some(final_success) = expected_final_success {
        if observation.get("final_success").and_then(Value::as_bool) != Some(final_success) {
            return false;
        }
    }
    true
}

fn normalize_category(category: &str) -> String {
    let mut normalized = String::new();
    let mut previous_was_lower_or_digit = false;
    for character in category.trim().chars() {
        if character.is_ascii_uppercase() && previous_was_lower_or_digit {
            normalized.push('_');
        }
        if character.is_ascii_alphanumeric() {
            normalized.push(character.to_ascii_lowercase());
            previous_was_lower_or_digit =
                character.is_ascii_lowercase() || character.is_ascii_digit();
        } else {
            normalized.push('_');
            previous_was_lower_or_digit = false;
        }
    }
    normalized
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_")
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

    #[test]
    fn observation_match_uses_normalized_category_and_final_success() {
        let observation = serde_json::json!({
            "category": "DiffConstraintFailed",
            "final_success": false
        });
        assert!(observation_matches_expected(
            &observation,
            Some("diff_constraint_failed"),
            Some(false)
        ));
        assert!(!observation_matches_expected(
            &observation,
            Some("verification_failed"),
            Some(false)
        ));
        assert!(!observation_matches_expected(
            &observation,
            Some("diff_constraint_failed"),
            Some(true)
        ));
    }
}
