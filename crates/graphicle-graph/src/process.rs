// Adapted from Graphify 0.9.95 (92741d16), Apache-2.0; see NOTICE.md.
// Upstream: graphify/extractors/fortran.py, graphify/llm.py
//! Native bounded subprocess execution for upstream command integrations.
use std::{
    cell::Cell,
    io::{self, Read, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use wait_timeout::ChildExt;

thread_local! {
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// A synchronous request's deadline. It stays on its blocking thread and is
/// restored on return or unwind; nested work can shorten it, never extend it.
pub struct DeadlineScope {
    previous: Option<Instant>,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

pub fn deadline_scope(deadline: Option<Instant>) -> DeadlineScope {
    let previous = DEADLINE.with(|slot| {
        let previous = slot.get();
        slot.set(match (previous, deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        });
        previous
    });
    DeadlineScope {
        previous,
        _thread: std::marker::PhantomData,
    }
}

impl Drop for DeadlineScope {
    fn drop(&mut self) {
        DEADLINE.with(|slot| slot.set(self.previous));
    }
}

pub fn check_deadline() -> io::Result<()> {
    bounded_timeout(Duration::MAX).map(|_| ())
}

/// Bound a subprocess or blocking storage wait by the current request budget.
pub fn bounded_timeout(timeout: Duration) -> io::Result<Duration> {
    DEADLINE.with(|slot| match slot.get() {
        None => Ok(timeout),
        Some(deadline) => deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .map(|left| left.min(timeout))
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "request deadline exceeded")),
    })
}

pub struct Output {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub fn capture(command: &mut Command, timeout: Duration) -> io::Result<Output> {
    capture_with_input(command, timeout, None)
}

pub fn capture_with_input(
    command: &mut Command,
    timeout: Duration,
    input: Option<Vec<u8>>,
) -> io::Result<Output> {
    capture_bounded(command, timeout, input, usize::MAX)
}

/// Drain both streams without retaining unbounded subprocess output.
pub fn capture_bounded(
    command: &mut Command,
    timeout: Duration,
    input: Option<Vec<u8>>,
    max_bytes: usize,
) -> io::Result<Output> {
    run(command, timeout, None, input, max_bytes).map(|lines| lines.output)
}

/// What [`capture_lines`] read, and whether it stopped the command.
pub struct Lines {
    pub output: Output,
    /// No stdout line arrived within the idle limit, so the command was
    /// killed; `output.stdout` holds the whole lines it printed before.
    pub stalled: bool,
}

/// [`capture_bounded`] for a command that reports progress one stdout line at
/// a time: `idle` bounds the wait for each next line, not the whole run, so a
/// command that keeps printing runs until it exits. The request deadline still
/// bounds it (a timeout error, as for [`capture_bounded`]).
pub fn capture_lines(
    command: &mut Command,
    idle: Duration,
    input: Option<Vec<u8>>,
    max_bytes: usize,
) -> io::Result<Lines> {
    run(command, Duration::MAX, Some(idle), input, max_bytes)
}

fn run(
    command: &mut Command,
    timeout: Duration,
    idle: Option<Duration>,
    input: Option<Vec<u8>>,
    max_bytes: usize,
) -> io::Result<Lines> {
    let timeout = bounded_timeout(timeout)?;
    let started = Instant::now();
    // When stdout last completed a line.
    let last_line = std::sync::Arc::new(std::sync::Mutex::new(started));
    fn drain(
        mut stream: impl Read,
        max: usize,
        last_line: Option<std::sync::Arc<std::sync::Mutex<Instant>>>,
    ) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 8192];
        let mut exceeded = false;
        loop {
            let n = stream.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            let keep = n.min(max.saturating_sub(bytes.len()));
            bytes.extend_from_slice(&buffer[..keep]);
            exceeded |= keep != n;
            if let Some(last) = &last_line
                && buffer[..n].contains(&b'\n')
            {
                *last.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
            }
        }
        if exceeded {
            Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "command output bound exceeded",
            ))
        } else {
            Ok(bytes)
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let spawn_limit = idle.map_or(timeout, |idle| idle.min(timeout));
    let mut child = loop {
        if started.elapsed() >= spawn_limit {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "command timed out"));
        }
        match command.spawn() {
            Ok(child) => break child,
            // A concurrent fork can briefly retain the writable descriptor of
            // a freshly materialized helper, even after this thread closed it.
            // No child ran on ETXTBSY; retry without extending its time budget.
            Err(error) if error.kind() == io::ErrorKind::ExecutableFileBusy => {
                let remaining = spawn_limit.saturating_sub(started.elapsed());
                std::thread::sleep(remaining.min(Duration::from_millis(10)));
            }
            Err(error) => return Err(error),
        }
    };
    let writer = input.map(|bytes| {
        let mut stdin = child.stdin.take().expect("piped stdin");
        std::thread::spawn(move || stdin.write_all(&bytes))
    });
    // Drain both pipes concurrently, including during the deadline wait.
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let progress = idle.map(|_| last_line.clone());
    let out = std::thread::spawn(move || drain(stdout, max_bytes, progress));
    let err = std::thread::spawn(move || drain(stderr, max_bytes, None));
    let mut stalled = false;
    let status = loop {
        let left = timeout.saturating_sub(started.elapsed());
        let Some(idle) = idle else {
            break child.wait_timeout(left);
        };
        let quiet = last_line
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .elapsed();
        if quiet >= idle {
            stalled = true;
            break Ok(None);
        }
        match child.wait_timeout(left.min(idle - quiet)) {
            Ok(None) if !left.is_zero() && started.elapsed() < timeout => continue,
            other => break other,
        }
    };
    if !matches!(status, Ok(Some(_))) {
        #[cfg(unix)]
        {
            // cpp can be a wrapper that spawns a compiler. Kill that command's
            // private process group so inherited output pipes cannot hang us.
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(child.id() as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    let mut stdout = out
        .join()
        .map_err(|_| io::Error::other("stdout reader panicked"))??;
    let stderr = err
        .join()
        .map_err(|_| io::Error::other("stderr reader panicked"))??;
    if let Some(writer) = writer {
        // communicate() tolerates a child closing stdin before consuming it.
        if let Err(error) = writer
            .join()
            .map_err(|_| io::Error::other("stdin writer panicked"))?
            && error.kind() != io::ErrorKind::BrokenPipe
            && matches!(status, Ok(Some(_)))
        {
            return Err(error);
        }
    }
    if stalled {
        // A line cut by the kill is not a line.
        stdout.truncate(
            stdout
                .iter()
                .rposition(|b| *b == b'\n')
                .map_or(0, |i| i + 1),
        );
        return Ok(Lines {
            output: Output {
                success: false,
                code: None,
                stdout,
                stderr,
            },
            stalled,
        });
    }
    let Some(status) = status? else {
        return Err(io::Error::new(io::ErrorKind::TimedOut, "command timed out"));
    };
    Ok(Lines {
        output: Output {
            success: status.success(),
            code: status.code(),
            stdout,
            stderr,
        },
        stalled,
    })
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::*;

    #[cfg(target_os = "linux")]
    fn busy_executable() -> (tempfile::TempDir, std::fs::File, Command) {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("cat");
        std::fs::copy("/bin/cat", &executable).unwrap();
        let writer = std::fs::OpenOptions::new()
            .write(true)
            .open(&executable)
            .unwrap();
        let mut command = Command::new(executable);
        assert_eq!(
            command.spawn().unwrap_err().kind(),
            io::ErrorKind::ExecutableFileBusy
        );
        (directory, writer, command)
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn transient_busy_executable_recovers_and_preserves_input() {
        let (_directory, writer, mut command) = busy_executable();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            drop(writer);
        });
        let output = capture_with_input(
            &mut command,
            Duration::from_secs(5),
            Some(b"recovered\n".to_vec()),
        );
        release.join().unwrap();
        let output = output.unwrap();
        assert!(output.success);
        assert_eq!(output.stdout, b"recovered\n");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn busy_executable_retry_respects_command_and_request_deadlines() {
        let (_directory, _writer, mut command) = busy_executable();
        for request_deadline in [false, true] {
            let started = Instant::now();
            let _scope =
                deadline_scope(request_deadline.then_some(started + Duration::from_millis(40)));
            let timeout = if request_deadline {
                Duration::from_secs(5)
            } else {
                Duration::from_millis(40)
            };
            assert_eq!(
                capture(&mut command, timeout).err().unwrap().kind(),
                io::ErrorKind::TimedOut
            );
            assert!(started.elapsed() < Duration::from_secs(2));
        }
    }

    #[test]
    #[cfg(unix)]
    fn a_command_that_keeps_printing_runs_past_its_idle_limit() {
        let started = Instant::now();
        let lines = capture_lines(
            Command::new("/bin/sh").args([
                "-c",
                "for i in 1 2 3 4 5 6; do echo line$i; sleep 0.2; done",
            ]),
            Duration::from_millis(600),
            None,
            1 << 20,
        )
        .unwrap();
        assert!(!lines.stalled);
        assert!(lines.output.success);
        assert!(started.elapsed() >= Duration::from_millis(1100));
        assert_eq!(
            String::from_utf8(lines.output.stdout).unwrap(),
            "line1\nline2\nline3\nline4\nline5\nline6\n"
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_command_that_stops_printing_is_killed_and_keeps_its_whole_lines() {
        let started = Instant::now();
        let lines = capture_lines(
            Command::new("/bin/sh").args(["-c", "echo first; printf partial; sleep 10 & wait"]),
            Duration::from_millis(300),
            None,
            1 << 20,
        )
        .unwrap();
        assert!(lines.stalled);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(String::from_utf8(lines.output.stdout).unwrap(), "first\n");
    }

    #[test]
    #[cfg(unix)]
    fn the_request_deadline_still_bounds_a_command_that_keeps_printing() {
        let started = Instant::now();
        let _request = deadline_scope(Some(started + Duration::from_millis(400)));
        let result = capture_lines(
            Command::new("/bin/sh").args(["-c", "while true; do echo tick; sleep 0.05; done"]),
            Duration::from_secs(5),
            None,
            1 << 20,
        );
        assert_eq!(result.err().unwrap().kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    #[cfg(unix)]
    fn missing_executable_is_not_retried() {
        let directory = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let result = capture(
            &mut Command::new(directory.path().join("missing")),
            Duration::from_secs(5),
        );
        assert_eq!(result.err().unwrap().kind(), io::ErrorKind::NotFound);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    #[cfg(unix)]
    fn request_deadline_bounds_children_and_does_not_leak_to_the_next_request() {
        let started = Instant::now();
        {
            let _request = deadline_scope(Some(started + Duration::from_millis(40)));
            let _nested = deadline_scope(Some(started + Duration::from_secs(30)));
            let result = capture(
                Command::new("/bin/sh").args(["-c", "sleep 10 & wait"]),
                Duration::from_secs(5),
            );
            assert_eq!(result.err().unwrap().kind(), io::ErrorKind::TimedOut);
            assert!(started.elapsed() < Duration::from_secs(2));
            assert_eq!(
                check_deadline().unwrap_err().kind(),
                io::ErrorKind::TimedOut
            );
        }
        let result = capture(
            Command::new("/bin/sh").args(["-c", "printf recovered"]),
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(result.success);
        assert_eq!(result.stdout, b"recovered");
    }
    #[test]
    #[cfg(unix)]
    fn output_limit_refuses_without_pipe_deadlock() {
        let result = capture_bounded(
            &mut Command::new("/bin/cat"),
            Duration::from_secs(5),
            Some(vec![b'x'; 100_000]),
            1024,
        );
        assert!(result.err().unwrap().to_string().contains("output bound"));
    }
    #[test]
    #[cfg(unix)]
    fn drains_both_streams_and_stops_timed_out_child() {
        let input = "stdin 😀 payload\n".repeat(20000).into_bytes();
        let echoed = capture_with_input(
            &mut Command::new("/bin/cat"),
            Duration::from_secs(5),
            Some(input.clone()),
        )
        .unwrap();
        assert!(echoed.success);
        assert_eq!(echoed.stdout, input);
        let result = capture(
            Command::new("/bin/sh").args([
                "-c",
                "i=0; while [ $i -lt 15000 ]; do echo out; echo err >&2; i=$((i+1)); done",
            ]),
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(result.success);
        assert_eq!(result.stdout.len(), 60000);
        assert_eq!(result.stderr.len(), 60000);
        assert_eq!(
            capture(
                Command::new("/bin/sleep").arg("2"),
                Duration::from_millis(30)
            )
            .err()
            .unwrap()
            .kind(),
            io::ErrorKind::TimedOut
        );
        let started = std::time::Instant::now();
        assert_eq!(
            capture(
                Command::new("/bin/sh").args(["-c", "sleep 10 & wait"]),
                Duration::from_millis(30)
            )
            .err()
            .unwrap()
            .kind(),
            io::ErrorKind::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
