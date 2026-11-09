//! One discipline for every helper process the app spawns (afconvert,
//! the AI CLIs): its own process group so a timeout or cancel takes forked
//! helpers down with it, stdin written from a thread so a child that exits
//! early cannot stall the writer, both output pipes drained concurrently
//! and bounded, and a deadline polled instead of an open-ended wait.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Captured output stops growing past this; the child is still drained so
/// it never blocks on a full pipe. Chat streams every tool result through
/// stdout, which is why this is generous.
const MAX_OUTPUT_BYTES: usize = 32 * 1024 * 1024;
const POLL: Duration = Duration::from_millis(50);

#[derive(Debug)]
pub(crate) struct Finished {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Finished {
    pub fn stdout_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

#[derive(Debug)]
pub(crate) enum RunError {
    /// The program could not be started at all.
    Spawn(std::io::Error),
    /// Killed at the deadline, together with its process group.
    TimedOut(Duration),
    /// Killed because the cancel flag was raised.
    Cancelled,
    Io(std::io::Error),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Spawn(error) => write!(f, "couldn't start the process: {error}"),
            RunError::TimedOut(timeout) => write!(f, "timed out after {timeout:?} and was killed"),
            RunError::Cancelled => write!(f, "cancelled"),
            RunError::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for RunError {}

/// How to run a command. `on_stdout_line` sees each stdout line as it
/// arrives (chat shows tool calls live); the whole output is still
/// returned afterwards.
pub(crate) struct Run<'a> {
    pub stdin: Option<&'a str>,
    pub timeout: Duration,
    pub cancel: Option<&'a AtomicBool>,
    pub on_stdout_line: Option<&'a mut dyn FnMut(&str)>,
}

impl Run<'_> {
    pub fn with_timeout(timeout: Duration) -> Self {
        Run {
            stdin: None,
            timeout,
            cancel: None,
            on_stdout_line: None,
        }
    }
}

pub(crate) fn run(cmd: Command, run: Run<'_>) -> Result<Finished, RunError> {
    run_capped(cmd, run, MAX_OUTPUT_BYTES)
}

fn run_capped(mut cmd: Command, mut run: Run<'_>, cap: usize) -> Result<Finished, RunError> {
    cmd.stdin(if run.stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    // Own process group: a kill must reach forked helpers that inherited
    // our pipes, or the reader threads would block until they exit.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(RunError::Spawn)?;

    // The prompt is written from its own thread and the error ignored: a
    // child that exits before reading it is reported through its exit
    // status, not by a broken pipe here, and it is always waited for.
    if let (Some(input), Some(mut stdin)) = (run.stdin, child.stdin.take()) {
        let input = input.as_bytes().to_vec();
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }

    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let (lines_tx, lines_rx) = mpsc::channel::<String>();
    let stream_lines = run.on_stdout_line.is_some();
    let stdout_reader = std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut all = Vec::new();
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if all.len() < cap {
                all.extend_from_slice(&line[..line.len().min(cap - all.len())]);
            }
            if stream_lines {
                let text = String::from_utf8_lossy(&line);
                let _ = lines_tx.send(text.trim_end_matches(['\n', '\r']).to_string());
            }
        }
        all
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut all = Vec::new();
        let mut chunk = [0u8; 8192];
        let mut reader = stderr;
        while let Ok(n) = reader.read(&mut chunk) {
            if n == 0 {
                break;
            }
            if all.len() < cap {
                all.extend_from_slice(&chunk[..n.min(cap - all.len())]);
            }
        }
        all
    });

    let started = Instant::now();
    let status = loop {
        if let Some(on_line) = run.on_stdout_line.as_deref_mut() {
            while let Ok(line) = lines_rx.try_recv() {
                on_line(&line);
            }
        }
        match child.try_wait().map_err(RunError::Io)? {
            Some(status) => break status,
            None => {
                let stop = if run.cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                    Some(RunError::Cancelled)
                } else if started.elapsed() >= run.timeout {
                    Some(RunError::TimedOut(run.timeout))
                } else {
                    None
                };
                if let Some(stop) = stop {
                    kill_group(&mut child);
                    let _ = stdout_reader.join();
                    let _ = stderr_reader.join();
                    return Err(stop);
                }
                std::thread::sleep(POLL);
            }
        }
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    if let Some(on_line) = run.on_stdout_line.as_deref_mut() {
        while let Ok(line) = lines_rx.try_recv() {
            on_line(&line);
        }
    }
    Ok(Finished {
        status,
        stdout,
        stderr,
    })
}

/// Kills the child's whole process group, then the child itself as a
/// fallback, and reaps it. Best effort: if the kill fails the process is
/// already gone, and the caller reports the stop either way.
fn kill_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    // SAFETY: kill(2) with a negative pid targets the process group the
    // child was started in (`process_group(0)` above); it touches no memory.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Command {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", script]);
        cmd
    }

    #[test]
    fn captures_both_streams_and_the_exit_status() {
        let finished = run(sh("printf out; printf err >&2; exit 3"), Run::with_timeout(Duration::from_secs(5))).unwrap();
        assert_eq!(finished.status.code(), Some(3));
        assert_eq!(finished.stdout_lossy(), "out");
        assert_eq!(finished.stderr_lossy(), "err");
    }

    /// A child that exits without reading a prompt larger than the pipe
    /// buffer used to fail the stdin write and leave a zombie behind.
    #[test]
    fn a_child_that_ignores_a_large_stdin_is_still_reaped_and_reported() {
        let prompt = "x".repeat(512 * 1024);
        let finished = run(
            sh("exit 7"),
            Run {
                stdin: Some(&prompt),
                timeout: Duration::from_secs(5),
                cancel: None,
                on_stdout_line: None,
            },
        )
        .unwrap();
        assert_eq!(finished.status.code(), Some(7));
    }

    #[test]
    fn streams_stdout_lines_and_caps_the_captured_output() {
        let mut seen = 0usize;
        let finished = run_capped(
            sh("i=0; while [ $i -lt 2000 ]; do echo line$i; i=$((i+1)); done"),
            Run {
                stdin: None,
                timeout: Duration::from_secs(10),
                cancel: None,
                on_stdout_line: Some(&mut |_| seen += 1),
            },
            4096,
        )
        .unwrap();
        assert!(finished.status.success());
        assert_eq!(seen, 2000);
        assert_eq!(finished.stdout.len(), 4096);
    }

    #[test]
    fn a_raised_cancel_flag_kills_the_process_tree() {
        let cancel = AtomicBool::new(false);
        let (tx, rx) = mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let started = Instant::now();
                let error = run(
                    sh("sleep 30 &\nwait"),
                    Run {
                        stdin: None,
                        timeout: Duration::from_secs(30),
                        cancel: Some(&cancel),
                        on_stdout_line: None,
                    },
                )
                .unwrap_err();
                let _ = tx.send((error, started.elapsed()));
            });
            std::thread::sleep(Duration::from_millis(200));
            cancel.store(true, Ordering::Relaxed);
        });
        let (error, elapsed) = rx.recv().unwrap();
        assert!(matches!(error, RunError::Cancelled), "{error}");
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    }
}
