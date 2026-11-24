//! What the context bar shows: the shell's working directory, the git state
//! there, and the servers the shell started (`node :5173`).
//!
//! Gathering it spawns git and walks the process and TCP tables, so it runs on
//! its own thread ([`Watcher`]) and reports only when something changed.

mod git;
#[cfg(windows)]
mod sys;
pub mod tasks;

use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

pub use git::{FileChange, Git};
pub use tasks::Task;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Context {
    pub cwd: Option<PathBuf>,
    /// The repository `cwd` is in: the project the sidebar groups it under.
    pub root: Option<PathBuf>,
    pub git: Option<Git>,
    pub servers: Vec<Server>,
    /// What the project at `root` can run.
    pub tasks: Vec<Task>,
}

/// A process under the shell that listens on a TCP port.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Server {
    pub port: u16,
    /// Executable name without `.exe`, such as `node` or `python`.
    pub name: String,
}

/// Servers can start or stop at any time, so they are polled. Process and TCP
/// tables are a few syscalls; this costs well under a millisecond.
const SERVER_POLL: Duration = Duration::from_secs(2);
/// `git status` reads the working tree, which costs real time on large repos,
/// so it runs less often unless the directory changes.
const GIT_POLL: Duration = Duration::from_secs(5);

/// Handle to the background thread. Dropping it stops the thread.
pub struct Watcher {
    cwd: Sender<PathBuf>,
}

impl Watcher {
    /// Watches the process tree under `shell_pid` and calls `on_change` from
    /// the watcher thread whenever the context differs from the last report.
    pub fn spawn(
        shell_pid: u32,
        cwd: Option<PathBuf>,
        on_change: impl Fn(Context) + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<PathBuf>();
        std::thread::Builder::new()
            .name("context".into())
            .spawn(move || {
                let mut context = Context {
                    cwd,
                    ..Context::default()
                };
                let mut last_git = None::<Instant>;
                let mut reported = None;
                loop {
                    let git_due = last_git.is_none_or(|t| t.elapsed() >= GIT_POLL);
                    if git_due {
                        context.git = context.cwd.as_deref().and_then(git::status);
                        context.root = context.cwd.as_deref().and_then(git::root);
                        let project = context.root.as_deref().or(context.cwd.as_deref());
                        context.tasks = project.map(tasks::detect).unwrap_or_default();
                        last_git = Some(Instant::now());
                    }
                    context.servers = servers(shell_pid);
                    if reported.as_ref() != Some(&context) {
                        on_change(context.clone());
                        reported = Some(context.clone());
                    }
                    match rx.recv_timeout(SERVER_POLL) {
                        Ok(cwd) => {
                            context.cwd = Some(cwd);
                            last_git = None;
                            // The UI blanks git while it waits for this answer,
                            // so send it even if nothing differs.
                            reported = None;
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            })
            .expect("spawning the context thread");
        Self { cwd: tx }
    }

    /// The shell reported a new working directory.
    pub fn set_cwd(&self, cwd: PathBuf) {
        let _ = self.cwd.send(cwd);
    }
}

#[cfg(windows)]
fn servers(shell_pid: u32) -> Vec<Server> {
    sys::servers(shell_pid)
}

#[cfg(not(windows))]
fn servers(_shell_pid: u32) -> Vec<Server> {
    Vec::new()
}

/// Every pid below `root` in a (pid, parent pid) list, `root` excluded.
fn descendants(root: u32, processes: &[(u32, u32)]) -> Vec<u32> {
    let mut found = Vec::new();
    let mut frontier = vec![root];
    while let Some(parent) = frontier.pop() {
        for &(pid, ppid) in processes {
            // A pid is its own parent only for the idle process; guard loops anyway.
            if ppid == parent && pid != parent && pid != root && !found.contains(&pid) {
                found.push(pid);
                frontier.push(pid);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descendants_follow_the_whole_tree() {
        let processes = [(10, 1), (11, 10), (12, 11), (13, 10), (20, 1), (21, 20)];
        let mut found = descendants(10, &processes);
        found.sort();
        assert_eq!(found, [11, 12, 13]);
    }

    /// The UI clears what it shows when the shell reports a directory, so the
    /// watcher must answer every report, even when nothing it knows changed.
    #[test]
    fn every_reported_directory_gets_an_answer() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let (tx, rx) = mpsc::channel();
        let watcher = Watcher::spawn(u32::MAX, Some(dir.clone()), move |context| {
            let _ = tx.send(context);
        });
        let wait = || rx.recv_timeout(Duration::from_secs(10)).expect("a report");
        let first = wait();
        watcher.set_cwd(dir);
        assert_eq!(wait(), first);
    }

    #[test]
    fn descendants_survive_cycles() {
        let processes = [(10, 11), (11, 10)];
        assert_eq!(descendants(10, &processes), [11]);
    }
}
