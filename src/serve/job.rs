//! The one benchmark run the server may have in flight. The run is a child
//! `cpubench run` process: a crash in a kernel (the build aborts on panic)
//! cannot take the web server down, cancelling is just killing the child, and
//! the `.partial.json` the child already writes after every kernel is the
//! progress channel. Anything that goes wrong is reported with the child's
//! own stderr tail, never swallowed.

use crate::model::KernelResult;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const LOG_LINES: usize = 200;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Phase {
    Idle,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Running => "running",
            Phase::Done => "done",
            Phase::Failed => "failed",
            Phase::Cancelled => "cancelled",
        }
    }
}

pub struct Job {
    pub phase: Phase,
    pub label: String,
    pub out: PathBuf,
    started: Option<Instant>,
    finished_after: Option<Duration>,
    pub log: VecDeque<String>,
    child: Option<Child>,
    cancelled: bool,
    pub error: Option<String>,
}

pub type Shared = Arc<Mutex<Job>>;

pub fn new_shared(out: PathBuf) -> Shared {
    Arc::new(Mutex::new(Job {
        phase: Phase::Idle,
        label: String::new(),
        out,
        started: None,
        finished_after: None,
        log: VecDeque::new(),
        child: None,
        cancelled: false,
        error: None,
    }))
}

pub fn partial_path(out: &Path, label: &str) -> PathBuf {
    out.join(format!("{label}.partial.json"))
}

/// Spawn `cpubench run <args> --out <out>`. Fails with a message if a run is already active.
pub fn start(shared: &Shared, label: &str, args: &[String]) -> Result<(), String> {
    let mut job = shared.lock().unwrap();
    if job.phase == Phase::Running {
        return Err(format!("a run ('{}') is already in progress; wait for it or cancel it", job.label));
    }
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate own executable: {e}"))?;
    let mut cmd = Command::new(exe);
    cmd.args(args).arg("--out").arg(&job.out).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: only async-signal-safe prctl between fork and exec. The child
        // must not outlive the server (it would keep loading the machine).
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                Ok(())
            });
        }
    }
    let mut child = cmd.spawn().map_err(|e| format!("cannot start benchmark: {e}"))?;
    let stderr = child.stderr.take();

    job.phase = Phase::Running;
    job.label = label.to_string();
    job.started = Some(Instant::now());
    job.finished_after = None;
    job.log.clear();
    job.cancelled = false;
    job.error = None;
    job.child = Some(child);
    drop(job);

    if let Some(stderr) = stderr {
        let shared = shared.clone();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut j = shared.lock().unwrap();
                if j.log.len() >= LOG_LINES {
                    j.log.pop_front();
                }
                j.log.push_back(line);
            }
        });
    }
    let shared = shared.clone();
    let label = label.to_string();
    thread::spawn(move || reap(shared, label));
    Ok(())
}

fn tail(log: &VecDeque<String>, n: usize) -> String {
    let skip = log.len().saturating_sub(n);
    log.iter().skip(skip).cloned().collect::<Vec<_>>().join("\n")
}

/// Poll the child until it exits, then record how it ended.
fn reap(shared: Shared, label: String) {
    loop {
        thread::sleep(Duration::from_millis(400));
        let mut job = shared.lock().unwrap();
        let Some(child) = job.child.as_mut() else { return };
        match child.try_wait() {
            Ok(None) => continue,
            Ok(Some(status)) => {
                // Let the stderr reader drain what the child printed last.
                drop(job);
                thread::sleep(Duration::from_millis(150));
                let mut job = shared.lock().unwrap();
                job.child = None;
                job.finished_after = job.started.map(|s| s.elapsed());
                let report = job.out.join(format!("{label}.json"));
                if job.cancelled {
                    job.phase = Phase::Cancelled;
                    let _ = std::fs::remove_file(partial_path(&job.out, &label));
                } else if status.success() && report.is_file() {
                    job.phase = Phase::Done;
                } else {
                    job.phase = Phase::Failed;
                    let how = if status.success() { "exited normally but wrote no report".to_string() } else { format!("{status}") };
                    job.error = Some(format!("benchmark process ended abnormally ({how}). Last output:\n{}", tail(&job.log, 15)));
                }
                return;
            }
            Err(e) => {
                job.child = None;
                job.phase = Phase::Failed;
                job.error = Some(format!("lost track of the benchmark process: {e}"));
                return;
            }
        }
    }
}

/// Kill the running child. Returns false if nothing was running.
pub fn cancel(shared: &Shared) -> bool {
    let mut job = shared.lock().unwrap();
    if job.phase != Phase::Running {
        return false;
    }
    job.cancelled = true;
    if let Some(child) = job.child.as_mut() {
        let _ = child.kill();
    }
    true
}

pub struct View {
    pub phase: Phase,
    pub label: String,
    pub elapsed_secs: f64,
    pub error: Option<String>,
    pub log_tail: String,
}

pub fn view(shared: &Shared) -> View {
    let job = shared.lock().unwrap();
    let elapsed = match (job.phase, job.started, job.finished_after) {
        (Phase::Running, Some(s), _) => s.elapsed(),
        (_, _, Some(f)) => f,
        _ => Duration::ZERO,
    };
    View {
        phase: job.phase,
        label: job.label.clone(),
        elapsed_secs: elapsed.as_secs_f64(),
        error: job.error.clone(),
        log_tail: tail(&job.log, 25),
    }
}

/// Kernel results the child has finished so far (empty if none yet).
pub fn read_partial(out: &Path, label: &str) -> Vec<KernelResult> {
    std::fs::read_to_string(partial_path(out, label))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_server_refuses_a_second_run() {
        let shared = new_shared(std::env::temp_dir());
        shared.lock().unwrap().phase = Phase::Running;
        shared.lock().unwrap().label = "first".into();
        let err = start(&shared, "second", &["run".into()]).unwrap_err();
        assert!(err.contains("already in progress") && err.contains("first"));
    }

    #[test]
    fn cancel_without_a_run_is_a_noop() {
        assert!(!cancel(&new_shared(std::env::temp_dir())));
    }

    #[test]
    fn log_tail_keeps_the_last_lines() {
        let log: VecDeque<String> = (1..=5).map(|i| format!("l{i}")).collect();
        assert_eq!(tail(&log, 2), "l4\nl5");
        assert_eq!(tail(&log, 99).lines().count(), 5);
    }

    #[test]
    fn partial_results_tolerate_missing_or_corrupt_files() {
        let dir = std::env::temp_dir().join(format!("cb-job-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read_partial(&dir, "nothing").is_empty());
        std::fs::write(partial_path(&dir, "bad"), "{not json").unwrap();
        assert!(read_partial(&dir, "bad").is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
