use anyhow::{Context, Result};
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const DEFAULT_LOCK_STALE_AFTER: Duration = Duration::from_secs(10 * 60);

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
    let mut command = Command::new(&exe);
    command
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_child_process_group(&mut command);
    let mut child = command
        .spawn()
        .with_context(|| format!("run {} {}", exe.display(), args.join(" ")))?;
    let mut stdout_reader = child.stdout.take().map(read_child_stream);
    let mut stderr_reader = child.stderr.take().map(read_child_stream);
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().context("poll air child process")? {
            let stdout = join_child_stream(stdout_reader.take(), "stdout")?;
            let stderr = join_child_stream(stderr_reader.take(), "stderr")?;
            write_stage_log(log_path, args, &stdout, &stderr)?;
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
            let _ = child.wait();
            let stdout = join_child_stream(stdout_reader.take(), "stdout")?;
            let stderr = join_child_stream(stderr_reader.take(), "stderr")?;
            write_stage_log(log_path, args, &stdout, &stderr)?;
            return Ok(StageResult {
                status: "timeout".to_string(),
                success: false,
                code: None,
            });
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

pub(crate) fn run_current_air_passthrough(
    cwd: &Path,
    args: &[std::ffi::OsString],
    timeout: Duration,
) -> Result<StageResult> {
    let exe = std::env::current_exe().context("resolve current air executable")?;
    if timeout.is_zero() {
        return Ok(StageResult {
            status: "timeout_before_start".to_string(),
            success: false,
            code: None,
        });
    }
    let mut command = Command::new(&exe);
    command
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("AIR_RUN_TIMEOUT_CHILD", "1");
    configure_child_process_group(&mut command);
    let mut child = command
        .spawn()
        .with_context(|| format!("run timed {} {:?}", exe.display(), args))?;
    let mut stdout = child.stdout.take().map(passthrough_child_stream);
    let mut stderr = child.stderr.take().map(passthrough_child_stderr);
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().context("poll timed air child process")? {
            join_passthrough_stream(stdout.take(), "stdout")?;
            join_passthrough_stream(stderr.take(), "stderr")?;
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
        if started.elapsed() >= timeout {
            terminate_child_gracefully(&mut child);
            let _ = child.wait();
            join_passthrough_stream(stdout.take(), "stdout")?;
            join_passthrough_stream(stderr.take(), "stderr")?;
            return Ok(StageResult {
                status: "timeout".to_string(),
                success: false,
                code: None,
            });
        }
        std::thread::sleep(Duration::from_millis(100));
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
        signal_child_process_group(child.id(), unix_process::SIGTERM);
        for _ in 0..12 {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        signal_child_process_group(child.id(), unix_process::SIGKILL);
    }
    let _ = child.kill();
}

fn configure_child_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| unix_process::set_current_process_group());
        }
    }
}

#[cfg(unix)]
fn signal_child_process_group(pid: u32, signal: std::os::raw::c_int) {
    unix_process::signal_process_group(pid, signal);
}

#[cfg(unix)]
mod unix_process {
    use std::io;
    use std::os::raw::c_int;

    pub(crate) const SIGTERM: c_int = 15;
    pub(crate) const SIGKILL: c_int = 9;

    unsafe extern "C" {
        fn kill(pid: c_int, sig: c_int) -> c_int;
        fn setpgid(pid: c_int, pgid: c_int) -> c_int;
    }

    pub(crate) fn set_current_process_group() -> io::Result<()> {
        let result = unsafe { setpgid(0, 0) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub(crate) fn signal_process_group(pid: u32, signal: c_int) {
        let group = -(pid as c_int);
        let _ = unsafe { kill(group, signal) };
    }
}

fn read_child_stream<R: Read + Send + 'static>(
    mut stream: R,
) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

fn join_child_stream(
    reader: Option<std::thread::JoinHandle<std::io::Result<Vec<u8>>>>,
    stream_name: &str,
) -> Result<Vec<u8>> {
    let Some(reader) = reader else {
        return Ok(Vec::new());
    };
    reader
        .join()
        .map_err(|_| anyhow::anyhow!("air child {stream_name} reader panicked"))?
        .with_context(|| format!("read air child {stream_name}"))
}

fn passthrough_child_stream<R: Read + Send + 'static>(
    mut stream: R,
) -> std::thread::JoinHandle<std::io::Result<()>> {
    std::thread::spawn(move || {
        let mut stdout = std::io::stdout().lock();
        std::io::copy(&mut stream, &mut stdout)?;
        stdout.flush()
    })
}

fn passthrough_child_stderr<R: Read + Send + 'static>(
    mut stream: R,
) -> std::thread::JoinHandle<std::io::Result<()>> {
    std::thread::spawn(move || {
        let mut stderr = std::io::stderr().lock();
        std::io::copy(&mut stream, &mut stderr)?;
        stderr.flush()
    })
}

fn join_passthrough_stream(
    reader: Option<std::thread::JoinHandle<std::io::Result<()>>>,
    stream_name: &str,
) -> Result<()> {
    let Some(reader) = reader else {
        return Ok(());
    };
    reader
        .join()
        .map_err(|_| anyhow::anyhow!("timed air child {stream_name} reader panicked"))?
        .with_context(|| format!("stream timed air child {stream_name}"))
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
                    if lock_is_stale(path, DEFAULT_LOCK_STALE_AFTER) {
                        // TODO: replace this sentinel-file lock with an advisory fd lock.
                        // This stale cleanup has a small stat-then-unlink TOCTOU window.
                        let _ = fs::remove_file(path);
                        continue;
                    }
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

fn lock_is_stale(path: &Path, stale_after: Duration) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    modified
        .elapsed()
        .map(|age| age >= stale_after)
        .unwrap_or(false)
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
