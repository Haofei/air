use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const DREAM_STATE_PATH: &str = ".air/dream/state.json";

pub(crate) struct DreamRunOptions {
    pub(crate) from: Vec<PathBuf>,
    pub(crate) since_unix: Option<u64>,
    pub(crate) limit: Option<usize>,
    pub(crate) out_dir: Option<PathBuf>,
    pub(crate) write_regressions: bool,
    pub(crate) full: bool,
}

pub(crate) struct DreamStateOptions;

#[derive(Debug, Serialize, Deserialize)]
struct DreamState {
    schema: String,
    status: String,
    state_file: String,
    last_started_at_unix: Option<u64>,
    last_completed_at_unix: Option<u64>,
    last_out_dir: Option<String>,
    last_roots: Vec<String>,
    last_findings: usize,
    last_top_finding: Option<DreamFindingSummary>,
}

#[derive(Debug, Serialize)]
struct DreamRunOutput {
    schema: &'static str,
    status: String,
    incremental: bool,
    since_unix: Option<u64>,
    state_file: String,
    out_dir: String,
    audit: DreamStage,
    improve: DreamStage,
    window_inputs: usize,
    findings: usize,
    top_finding: Option<DreamFindingSummary>,
    dream_json: String,
    dream_report: String,
    next_commands: Vec<String>,
}

#[derive(Debug, Serialize)]
struct DreamStage {
    status: String,
    out_dir: String,
    report: String,
    log: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DreamFindingSummary {
    id: String,
    category: String,
    summary: String,
    impact_score: i64,
}

pub(crate) fn run_dream(options: DreamRunOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let started_at = unix_now();
    let state_file = cwd.join(DREAM_STATE_PATH);
    let prior_state = read_dream_state(&state_file)?;
    let since_unix = options.since_unix.or_else(|| {
        if options.full {
            None
        } else {
            prior_state
                .as_ref()
                .and_then(|state| state.last_completed_at_unix)
        }
    });
    let incremental = !options.full;
    let roots = if options.from.is_empty() {
        vec![PathBuf::from("target/generated")]
    } else {
        options.from.clone()
    };
    let out_dir = absolutize(
        &cwd,
        options
            .out_dir
            .unwrap_or_else(|| PathBuf::from(".air/dream/latest")),
    );
    let audit_dir = out_dir.join("audit");
    let improve_dir = out_dir.join("improve");
    let log_dir = out_dir.join("logs");
    fs::create_dir_all(&log_dir).with_context(|| format!("create {}", log_dir.display()))?;
    let improve_roots = collect_window_improve_inputs(&cwd, &roots, since_unix, options.limit)?;
    let effective_improve_roots = if improve_roots.is_empty() {
        let empty = out_dir.join("window").join("empty");
        fs::create_dir_all(&empty).with_context(|| format!("create {}", empty.display()))?;
        vec![empty]
    } else {
        improve_roots.clone()
    };

    let audit_report = audit_dir.join("report.md");
    let mut audit_args = vec!["audit".to_string(), "collect".to_string()];
    for root in &roots {
        audit_args.push("--from".to_string());
        audit_args.push(root.display().to_string());
    }
    if let Some(since_unix) = options.since_unix {
        audit_args.push("--since-unix".to_string());
        audit_args.push(since_unix.to_string());
    }
    if let Some(limit) = options.limit {
        audit_args.push("--limit".to_string());
        audit_args.push(limit.to_string());
    }
    audit_args.push("--out-dir".to_string());
    audit_args.push(audit_dir.display().to_string());
    audit_args.push("--report".to_string());
    audit_args.push(audit_report.display().to_string());
    run_air_stage(&cwd, &audit_args, &log_dir.join("audit.log"))?;

    let improve_report = improve_dir.join("report.md");
    let mut improve_args = vec!["improve".to_string()];
    for root in &effective_improve_roots {
        improve_args.push("--from".to_string());
        improve_args.push(root.display().to_string());
    }
    improve_args.push("--out-dir".to_string());
    improve_args.push(improve_dir.display().to_string());
    if options.write_regressions {
        improve_args.push("--write-regressions".to_string());
    }
    run_air_stage(&cwd, &improve_args, &log_dir.join("improve.log"))?;

    let findings_file = improve_dir.join("findings.json");
    let findings = read_findings(&findings_file)?;
    let top_finding = findings.first().map(finding_summary);
    let next_commands = dream_next_commands(
        &roots,
        &out_dir,
        options.write_regressions,
        top_finding.as_ref(),
    );
    let dream_json = out_dir.join("dream.json");
    let dream_report = out_dir.join("dream.md");
    let output = DreamRunOutput {
        schema: "air.dream.v1",
        status: if findings.is_empty() {
            "no_findings".to_string()
        } else {
            "findings_ready".to_string()
        },
        incremental,
        since_unix,
        state_file: state_file.display().to_string(),
        out_dir: out_dir.display().to_string(),
        audit: DreamStage {
            status: "completed".to_string(),
            out_dir: audit_dir.display().to_string(),
            report: audit_report.display().to_string(),
            log: log_dir.join("audit.log").display().to_string(),
        },
        improve: DreamStage {
            status: "completed".to_string(),
            out_dir: improve_dir.display().to_string(),
            report: improve_report.display().to_string(),
            log: log_dir.join("improve.log").display().to_string(),
        },
        window_inputs: improve_roots.len(),
        findings: findings.len(),
        top_finding: top_finding.clone(),
        dream_json: dream_json.display().to_string(),
        dream_report: dream_report.display().to_string(),
        next_commands,
    };
    fs::write(&dream_json, serde_json::to_string_pretty(&output)?)
        .with_context(|| format!("write {}", dream_json.display()))?;
    write_dream_report(&dream_report, &output)?;
    let completed_at = unix_now();
    write_dream_state(
        &state_file,
        &DreamState {
            schema: "air.dream_state.v1".to_string(),
            status: output.status.clone(),
            state_file: state_file.display().to_string(),
            last_started_at_unix: Some(started_at),
            last_completed_at_unix: Some(completed_at),
            last_out_dir: Some(out_dir.display().to_string()),
            last_roots: roots
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
            last_findings: output.findings,
            last_top_finding: output.top_finding.clone(),
        },
    )?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

pub(crate) fn show_dream_state(_options: DreamStateOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let state_file = cwd.join(DREAM_STATE_PATH);
    let state = read_dream_state(&state_file)?.unwrap_or_else(|| DreamState {
        schema: "air.dream_state.v1".to_string(),
        status: "never_run".to_string(),
        state_file: state_file.display().to_string(),
        last_started_at_unix: None,
        last_completed_at_unix: None,
        last_out_dir: None,
        last_roots: Vec::new(),
        last_findings: 0,
        last_top_finding: None,
    });
    println!("{}", serde_json::to_string_pretty(&state)?);
    Ok(())
}

fn run_air_stage(cwd: &Path, args: &[String], log_path: &Path) -> Result<()> {
    let exe = std::env::current_exe().context("resolve current air executable")?;
    let output = Command::new(&exe)
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("run {} {}", exe.display(), args.join(" ")))?;
    let mut log = String::new();
    log.push_str("$ air ");
    log.push_str(&args.join(" "));
    log.push_str("\n\n## stdout\n\n");
    log.push_str(&String::from_utf8_lossy(&output.stdout));
    log.push_str("\n\n## stderr\n\n");
    log.push_str(&String::from_utf8_lossy(&output.stderr));
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(log_path, log).with_context(|| format!("write {}", log_path.display()))?;
    if !output.status.success() {
        bail!(
            "dream stage failed: air {} (status {:?}); see {}",
            args.join(" "),
            output.status.code(),
            log_path.display()
        );
    }
    Ok(())
}

fn read_findings(path: &Path) -> Result<Vec<Value>> {
    let value: Value = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("read {}", path.display()))?,
    )
    .with_context(|| format!("parse {}", path.display()))?;
    Ok(value
        .get("findings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn collect_window_improve_inputs(
    cwd: &Path,
    roots: &[PathBuf],
    since_unix: Option<u64>,
    limit: Option<usize>,
) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for root in roots {
        collect_improve_input_files(&absolutize(cwd, root.clone()), &mut files)?;
    }
    files.sort_by_key(|path| std::cmp::Reverse(file_mtime_unix(path)));
    files.dedup();
    if let Some(since_unix) = since_unix {
        files.retain(|path| file_mtime_unix(path).unwrap_or(0) >= since_unix);
    }
    if let Some(limit) = limit {
        files.truncate(limit);
    }
    Ok(files)
}

fn collect_improve_input_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    if root.is_file() {
        if is_improve_input(root) {
            files.push(root.to_path_buf());
        }
        return Ok(());
    }
    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry.with_context(|| format!("read entry under {}", root.display()))?;
        let path = entry.path();
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if path.is_dir() {
            if matches!(
                file_name,
                ".git" | "node_modules" | ".venv" | ".cache" | "tmp"
            ) {
                continue;
            }
            collect_improve_input_files(&path, files)?;
        } else if is_improve_input(&path) {
            files.push(path);
        }
    }
    Ok(())
}

fn is_improve_input(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("run.json" | "artifact.json")
    )
}

fn finding_summary(value: &Value) -> DreamFindingSummary {
    DreamFindingSummary {
        id: string_field(value, "id").unwrap_or_else(|| "unknown".to_string()),
        category: string_field(value, "category").unwrap_or_else(|| "unknown".to_string()),
        summary: string_field(value, "summary").unwrap_or_else(|| "No summary".to_string()),
        impact_score: value
            .get("impact_score")
            .and_then(Value::as_i64)
            .unwrap_or(0),
    }
}

fn dream_next_commands(
    roots: &[PathBuf],
    out_dir: &Path,
    wrote_regressions: bool,
    top_finding: Option<&DreamFindingSummary>,
) -> Vec<String> {
    let mut commands = vec![
        format!(
            "air audit collect --from {} --out-dir {}",
            roots_display(roots),
            out_dir.join("audit").display()
        ),
        format!(
            "air improve --from {} --out-dir {}",
            roots_display(roots),
            out_dir.join("improve").display()
        ),
    ];
    if let Some(finding) = top_finding {
        if !wrote_regressions {
            commands.push(format!(
                "air improve --out-dir {} promote {}",
                out_dir.join("improve").display(),
                finding.id
            ));
        }
        let regression_file = out_dir
            .join("improve")
            .join("suggested-regressions")
            .join(format!("{}.json", finding.id.to_ascii_lowercase()));
        commands.push(format!(
            "air self fix {} --from {} --out-dir {} --regression-file {} --evaluate",
            finding.id,
            roots_display(roots),
            out_dir.join("candidates").display(),
            regression_file.display()
        ));
        commands.push(format!(
            "air self compare {} --from {} --out {}",
            finding.id,
            out_dir.join("candidates").join(&finding.id).display(),
            out_dir
                .join("candidates")
                .join(format!("{}.md", finding.id))
                .display()
        ));
    }
    commands
}

fn write_dream_report(path: &Path, output: &DreamRunOutput) -> Result<()> {
    let mut out = String::new();
    out.push_str("# AIR Dream Report\n\n");
    out.push_str("Dream is an offline, auditable optimization cycle. It reviews recent run artifacts, mines real failure patterns, and prepares safe next steps without changing source code automatically.\n\n");
    out.push_str(&format!("- status: `{}`\n", output.status));
    out.push_str(&format!("- incremental: `{}`\n", output.incremental));
    out.push_str(&format!("- since_unix: `{:?}`\n", output.since_unix));
    out.push_str(&format!("- window_inputs: `{}`\n", output.window_inputs));
    out.push_str(&format!("- findings: `{}`\n", output.findings));
    out.push_str(&format!("- state file: `{}`\n", output.state_file));
    out.push_str(&format!("- audit report: `{}`\n", output.audit.report));
    out.push_str(&format!(
        "- improve report: `{}`\n\n",
        output.improve.report
    ));
    if let Some(finding) = &output.top_finding {
        out.push_str("## Top Finding\n\n");
        out.push_str(&format!("- id: `{}`\n", finding.id));
        out.push_str(&format!("- category: `{}`\n", finding.category));
        out.push_str(&format!("- impact_score: `{}`\n", finding.impact_score));
        out.push_str(&format!("- summary: {}\n\n", finding.summary));
    }
    out.push_str("## Next Commands\n\n");
    if output.next_commands.is_empty() {
        out.push_str("No follow-up commands.\n");
    } else {
        for command in &output.next_commands {
            out.push_str("```bash\n");
            out.push_str(command);
            out.push_str("\n```\n\n");
        }
    }
    fs::write(path, out).with_context(|| format!("write {}", path.display()))
}

fn roots_display(roots: &[PathBuf]) -> String {
    roots
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(" --from ")
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn read_dream_state(path: &Path) -> Result<Option<DreamState>> {
    if !path.exists() {
        return Ok(None);
    }
    let value = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("read {}", path.display()))?,
    )
    .with_context(|| format!("parse {}", path.display()))?;
    Ok(Some(value))
}

fn write_dream_state(path: &Path, state: &DreamState) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(path, serde_json::to_string_pretty(state)?)
        .with_context(|| format!("write {}", path.display()))
}

fn file_mtime_unix(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn absolutize(cwd: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}
