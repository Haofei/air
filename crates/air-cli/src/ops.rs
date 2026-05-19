use anyhow::{Context, Result};
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub(crate) struct AirLayout {
    cwd: PathBuf,
}

impl AirLayout {
    pub(crate) fn new(cwd: impl Into<PathBuf>) -> Self {
        Self { cwd: cwd.into() }
    }

    pub(crate) fn memory_dir(&self) -> PathBuf {
        self.cwd.join(".air/memory")
    }

    pub(crate) fn dream_dir(&self) -> PathBuf {
        self.cwd.join(".air/dream")
    }

    pub(crate) fn dream_findings_path(&self) -> PathBuf {
        self.dream_dir().join("findings.jsonl")
    }

    pub(crate) fn dream_ledger_path(&self) -> PathBuf {
        self.dream_dir().join("ledger.jsonl")
    }

    pub(crate) fn dream_state_path(&self) -> PathBuf {
        self.dream_dir().join("state.json")
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StageResult {
    pub(crate) status: String,
    pub(crate) success: bool,
    pub(crate) code: Option<i32>,
}

pub(crate) fn run_current_air_stage(
    cwd: &Path,
    args: &[String],
    log_path: &Path,
    timeout: Option<Duration>,
) -> Result<StageResult> {
    let exe = std::env::current_exe().context("resolve current air executable")?;
    if timeout.is_some_and(|timeout| timeout.is_zero()) {
        write_stage_timeout_log(log_path, args, "timeout_before_start")?;
        return Ok(StageResult {
            status: "timeout_before_start".to_string(),
            success: false,
            code: None,
        });
    }
    let mut child = Command::new(&exe)
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("run {} {}", exe.display(), args.join(" ")))?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().context("poll air child process")? {
            let output = child
                .wait_with_output()
                .context("collect air child output")?;
            write_stage_log(log_path, args, &output.stdout, &output.stderr)?;
            return Ok(StageResult {
                status: if status.success() {
                    "completed".to_string()
                } else {
                    format!("failed_status_{:?}", status.code())
                },
                success: status.success(),
                code: status.code(),
            });
        }
        if timeout.is_some_and(|timeout| started.elapsed() >= timeout) {
            terminate_child_gracefully(&mut child);
            let output = child
                .wait_with_output()
                .context("collect timed out air child output")?;
            write_stage_log(log_path, args, &output.stdout, &output.stderr)?;
            return Ok(StageResult {
                status: "timeout".to_string(),
                success: false,
                code: None,
            });
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

pub(crate) fn append_jsonl_locked<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    with_jsonl_lock(path, || {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("open {}", path.display()))?;
        writeln!(file, "{}", serde_json::to_string(value)?)
            .with_context(|| format!("write {}", path.display()))
    })
}

pub(crate) fn with_jsonl_lock<T>(path: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let _lock = FileLock::acquire(&lock_path(path), Duration::from_secs(30))?;
    f()
}

pub(crate) fn write_json_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let tmp = temp_sibling(path, "tmp");
    fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} to {}", tmp.display(), path.display()))
}

fn write_stage_log(log_path: &Path, args: &[String], stdout: &[u8], stderr: &[u8]) -> Result<()> {
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let mut log = String::new();
    log.push_str("$ air ");
    log.push_str(&args.join(" "));
    log.push_str("\n\n[stdout]\n");
    log.push_str(&String::from_utf8_lossy(stdout));
    log.push_str("\n\n[stderr]\n");
    log.push_str(&String::from_utf8_lossy(stderr));
    fs::write(log_path, log).with_context(|| format!("write {}", log_path.display()))
}

fn write_stage_timeout_log(log_path: &Path, args: &[String], status: &str) -> Result<()> {
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(
        log_path,
        format!("$ air {}\n\n[status]\n{status}\n", args.join(" ")),
    )
    .with_context(|| format!("write {}", log_path.display()))
}

fn terminate_child_gracefully(child: &mut Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(child.id().to_string())
            .status();
        for _ in 0..12 {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    let _ = child.kill();
}

fn lock_path(path: &Path) -> PathBuf {
    let mut lock = path.to_path_buf();
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("jsonl");
    lock.set_file_name(format!(".{name}.lock"));
    lock
}

fn temp_sibling(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("air-file");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    path.with_file_name(format!(
        ".{file_name}.{suffix}-{}-{nanos}",
        std::process::id()
    ))
}

struct FileLock {
    path: PathBuf,
}

impl FileLock {
    fn acquire(path: &Path, timeout: Duration) -> Result<Self> {
        let started = Instant::now();
        loop {
            match OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(_) => {
                    return Ok(Self {
                        path: path.to_path_buf(),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if started.elapsed() >= timeout {
                        anyhow::bail!("timed out waiting for lock {}", path.display());
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(error) => {
                    return Err(error).with_context(|| format!("create lock {}", path.display()));
                }
            }
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
