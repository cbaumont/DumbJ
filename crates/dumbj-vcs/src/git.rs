use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use crate::VcsError;

pub(crate) struct Output {
    pub(crate) success: bool,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

pub(crate) fn never() -> AtomicBool {
    AtomicBool::new(false)
}

/// Runs `git --no-optional-locks <args>` in `workdir` with a null stdin, so it never writes
/// to the repository or waits for input. `Ok(None)` when `cancelled` was set; the child is
/// killed then. A failing exit status is reported in the result, not as an error.
pub(crate) fn run_raw(
    workdir: &Path,
    args: &[&str],
    cancelled: &AtomicBool,
) -> Result<Option<Output>, VcsError> {
    run_raw_env(workdir, args, &[], cancelled)
}

/// `run_raw` with extra environment variables for the child.
pub(crate) fn run_raw_env(
    workdir: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    cancelled: &AtomicBool,
) -> Result<Option<Output>, VcsError> {
    run_child(workdir, args, env, None, cancelled)
}

/// `run`, with `input` written to the child's stdin.
pub(crate) fn run_input(
    workdir: &Path,
    args: &[&str],
    input: Vec<u8>,
    cancelled: &AtomicBool,
) -> Result<Option<Vec<u8>>, VcsError> {
    match run_child(workdir, args, &[], Some(input), cancelled)? {
        None => Ok(None),
        Some(out) if out.success => Ok(Some(out.stdout)),
        Some(out) => Err(VcsError::Git(
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        )),
    }
}

fn run_child(
    workdir: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    input: Option<Vec<u8>>,
    cancelled: &AtomicBool,
) -> Result<Option<Output>, VcsError> {
    let mut child = Command::new("git")
        .args(["--no-optional-locks", "-c", "log.showSignature=false"])
        .args(args)
        .envs(env.iter().copied())
        .current_dir(workdir)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let status = loop {
        if cancelled.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        thread::sleep(Duration::from_millis(2));
    };
    Ok(Some(Output {
        success: status.success(),
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    }))
}

/// Like `run_raw`, but a failing exit status is an error and only stdout is returned.
pub(crate) fn run(
    workdir: &Path,
    args: &[&str],
    cancelled: &AtomicBool,
) -> Result<Option<Vec<u8>>, VcsError> {
    match run_raw(workdir, args, cancelled)? {
        None => Ok(None),
        Some(out) if out.success => Ok(Some(out.stdout)),
        Some(out) => Err(VcsError::Git(
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        )),
    }
}

fn drain(pipe: Option<impl Read + Send + 'static>) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    })
}
