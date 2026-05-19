use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub(crate) struct StageResult {
    pub(crate) status: String,
    pub(crate) success: bool,
    pub(crate) code: Option<i32>,
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

fn terminate_child_gracefully(child: &mut Child) {
    #[cfg(unix)]
    {
        platform::signal_process_group(child.id(), platform::SIGTERM);
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(3) {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        platform::signal_process_group(child.id(), platform::SIGKILL);
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

fn configure_child_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(platform::set_current_process_group);
        }
    }
}

#[cfg(unix)]
mod platform {
    use std::io;
    use std::os::raw::c_int;

    pub(crate) const SIGTERM: c_int = 15;
    pub(crate) const SIGKILL: c_int = 9;

    unsafe extern "C" {
        fn setpgid(pid: i32, pgid: i32) -> i32;
        fn kill(pid: i32, signal: c_int) -> i32;
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
        let pgid = -(pid as i32);
        unsafe {
            let _ = kill(pgid, signal);
        }
    }
}
