use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

#[derive(Clone, Debug)]
pub struct ProjectTreeWatchEvent {
    pub root: PathBuf,
    pub generation: u64,
    pub paths: Vec<PathBuf>,
    pub rescan: bool,
    pub error: Option<String>,
}

pub struct ProjectTreeWatcher {
    receiver: Receiver<ProjectTreeWatchEvent>,
    cancelled: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    thread: Option<Thread>,
}

impl ProjectTreeWatcher {
    pub fn start(root: PathBuf, generation: u64) -> Result<Self, String> {
        let (sender, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let worker_root = root.clone();
        let worker = thread::Builder::new()
            .name("project-tree-watch".into())
            .spawn(move || watch(worker_root, generation, worker_cancelled, sender))
            .map_err(|error| format!("Cannot start project-tree watcher: {error}"))?;
        let thread = worker.thread().clone();
        Ok(Self {
            receiver,
            cancelled,
            worker: Some(worker),
            thread: Some(thread),
        })
    }

    pub fn poll(&mut self) -> Vec<ProjectTreeWatchEvent> {
        let events = self.receiver.try_iter().collect();
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            self.join();
        }
        events
    }

    pub fn request_cancel(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(thread) = &self.thread {
            thread.unpark();
        }
    }

    pub fn reap(&mut self) -> bool {
        if self.worker.is_none() {
            return true;
        }
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            self.join();
            true
        } else {
            false
        }
    }

    pub fn has_worker(&self) -> bool {
        self.worker.is_some()
    }

    fn join(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.thread = None;
    }
}

impl Drop for ProjectTreeWatcher {
    fn drop(&mut self) {
        self.request_cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn watch(
    root: PathBuf,
    generation: u64,
    cancelled: Arc<AtomicBool>,
    sender: Sender<ProjectTreeWatchEvent>,
) {
    let (events_tx, events_rx) = mpsc::channel();
    let mut watcher: RecommendedWatcher = match notify::recommended_watcher(move |event| {
        let _ = events_tx.send(event);
    }) {
        Ok(watcher) => watcher,
        Err(error) => {
            send_error(
                &sender,
                &root,
                generation,
                format!("Cannot watch project files: {error}"),
            );
            return;
        }
    };
    if let Err(error) = watcher.watch(&root, RecursiveMode::Recursive) {
        send_error(
            &sender,
            &root,
            generation,
            format!("Cannot watch project files: {error}"),
        );
        return;
    }

    let debounce = Duration::from_millis(200);
    let mut paths = HashSet::new();
    let mut rescan = false;
    let mut changed_at = None;
    while !cancelled.load(Ordering::Acquire) {
        let timeout = changed_at.map_or(Duration::from_secs(1), |at: Instant| {
            debounce
                .saturating_sub(at.elapsed())
                .min(Duration::from_millis(40))
        });
        match events_rx.recv_timeout(timeout) {
            Ok(Ok(event)) => {
                rescan |= event.need_rescan() || event.paths.is_empty();
                for path in event.paths {
                    if path.starts_with(&root) && !is_ignored(&root, &path) {
                        paths.insert(path);
                    }
                }
                changed_at = Some(Instant::now());
            }
            Ok(Err(error)) => send_error(
                &sender,
                &root,
                generation,
                format!("Project file watching failed: {error}"),
            ),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if changed_at.is_some_and(|at| at.elapsed() >= debounce) {
                    let changed_paths: Vec<_> = paths.drain().collect();
                    let needs_rescan = std::mem::take(&mut rescan);
                    if needs_rescan || !changed_paths.is_empty() {
                        let _ = sender.send(ProjectTreeWatchEvent {
                            root: root.clone(),
                            generation,
                            paths: changed_paths,
                            rescan: needs_rescan,
                            error: None,
                        });
                    }
                    changed_at = None;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn is_ignored(root: &std::path::Path, path: &std::path::Path) -> bool {
    path.strip_prefix(root).ok().is_some_and(|relative| {
        relative.components().any(|component| {
            let name = component.as_os_str();
            [".git", "target", "build", ".gradle"]
                .iter()
                .any(|ignored| name == *ignored)
        })
    })
}

fn send_error(
    sender: &Sender<ProjectTreeWatchEvent>,
    root: &std::path::Path,
    generation: u64,
    error: String,
) {
    let _ = sender.send(ProjectTreeWatchEvent {
        root: root.to_path_buf(),
        generation,
        paths: Vec::new(),
        rescan: false,
        error: Some(error),
    });
}
