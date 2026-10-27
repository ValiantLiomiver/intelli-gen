use std::path::PathBuf;
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant, SystemTime};

use super::{ProjectModel, maven_cache::{fingerprint_pom_paths, fingerprint_poms}};

#[derive(Clone, Debug)]
pub struct MavenPomEvent {
    pub root: PathBuf,
    pub project_generation: u64,
    pub model: Result<ProjectModel, String>,
}

pub struct MavenPomWatcher {
    receiver: Receiver<MavenPomEvent>,
    ready: Receiver<()>,
    cancelled: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    thread: Option<Thread>,
}

impl MavenPomWatcher {
    pub fn start(
        root: PathBuf,
        pom_paths: Vec<PathBuf>,
        project_generation: u64,
    ) -> Result<Self, String> {
        Self::start_with_timing(
            root,
            pom_paths,
            project_generation,
            Duration::from_millis(200),
            Duration::from_millis(800),
        )
    }

    fn start_with_timing(
        root: PathBuf,
        pom_paths: Vec<PathBuf>,
        project_generation: u64,
        poll_interval: Duration,
        debounce: Duration,
    ) -> Result<Self, String> {
        let (sender, receiver) = mpsc::channel();
        let (ready_sender, ready) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let worker = thread::Builder::new()
            .name("maven-pom-watch".into())
            .spawn(move || {
                watch(
                    root,
                    pom_paths,
                    project_generation,
                    poll_interval,
                    debounce,
                    worker_cancelled,
                    sender,
                    ready_sender,
                );
            })
            .map_err(|error| format!("Cannot start Maven POM watcher: {error}"))?;
        let thread = worker.thread().clone();
        Ok(Self {
            receiver,
            ready,
            cancelled,
            worker: Some(worker),
            thread: Some(thread),
        })
    }

    pub fn poll(&mut self) -> Vec<MavenPomEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.receiver.try_recv() {
            events.push(event);
        }
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            self.join();
        }
        events
    }

    pub fn cancel(&mut self) {
        self.request_cancel();
        self.join();
    }

    pub fn request_cancel(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(thread) = &self.thread {
            thread.unpark();
        }
    }

    /// Reap a stopped watcher without waiting for filesystem work on the caller.
    pub fn reap(&mut self) -> bool {
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

impl Drop for MavenPomWatcher {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn watch(
    root: PathBuf,
    mut pom_paths: Vec<PathBuf>,
    project_generation: u64,
    poll_interval: Duration,
    debounce: Duration,
    cancelled: Arc<AtomicBool>,
    sender: Sender<MavenPomEvent>,
    ready: Sender<()>,
) {
    const CONTENT_CHECK_INTERVAL: Duration = Duration::from_secs(5);
    pom_paths.push(root.join("pom.xml"));
    pom_paths.sort();
    pom_paths.dedup();
    let mut previous = metadata_snapshot(&pom_paths).ok();
    let mut previous_content = fingerprint_pom_paths(&root, &pom_paths).ok();
    let mut last_content_check = Instant::now();
    let _ = ready.send(());
    let mut changed_at = None;

    while !cancelled.load(Ordering::Acquire) {
        thread::park_timeout(poll_interval);
        if cancelled.load(Ordering::Acquire) {
            break;
        }
        let current = match metadata_snapshot(&pom_paths) {
            Ok(snapshot) => snapshot,
            Err(_) => continue,
        };
        if previous.as_ref() != Some(&current) {
            previous = Some(current);
            changed_at = Some(Instant::now());
        }
        if last_content_check.elapsed() >= CONTENT_CHECK_INTERVAL {
            last_content_check = Instant::now();
            if let Ok(fingerprint) = fingerprint_pom_paths(&root, &pom_paths)
                && previous_content.as_ref() != Some(&fingerprint)
            {
                previous_content = Some(fingerprint);
                changed_at = Some(Instant::now());
            }
        }
        if changed_at.is_some_and(|changed| changed.elapsed() >= debounce) {
            let model = ProjectModel::discover(&root).map_err(|error| error.to_string());
            if let Ok(model) = &model {
                pom_paths = project_pom_paths(model);
                previous = metadata_snapshot(&pom_paths).ok();
                previous_content = fingerprint_poms(model).ok();
            }
            let _ = sender.send(MavenPomEvent {
                root: root.clone(),
                project_generation,
                model,
            });
            changed_at = None;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PomMetadata {
    path: PathBuf,
    modified: Option<SystemTime>,
    length: Option<u64>,
}

fn metadata_snapshot(paths: &[PathBuf]) -> Result<Vec<PomMetadata>, String> {
    let mut paths = paths.to_vec();
    paths.sort();
    paths.dedup();
    paths.into_iter().map(|path| {
        match fs::metadata(&path) {
            Ok(metadata) => Ok(PomMetadata {
                path,
                modified: Some(metadata.modified().map_err(|error| format!("Cannot inspect POM timestamp: {error}"))?),
                length: Some(metadata.len()),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(PomMetadata { path, modified: None, length: None }),
            Err(error) => Err(format!("Cannot inspect Maven POM metadata: {error}")),
        }
    }).collect()
}

pub fn project_pom_paths(model: &ProjectModel) -> Vec<PathBuf> {
    let mut paths = vec![model.root.join("pom.xml")];
    paths.extend(
        model
            .modules
            .iter()
            .map(|module| module.root.join("pom.xml")),
    );
    paths.sort();
    paths.dedup();
    paths
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use super::MavenPomWatcher;

    fn project_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "intelli-gen-maven-watch-{}-{label}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("pom.xml"),
            "<project><modelVersion>4.0.0</modelVersion><name>initial</name></project>",
        )
        .unwrap();
        root
    }

    fn wait_ready(watcher: &MavenPomWatcher) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while watcher.ready.try_recv().is_err() {
            assert!(Instant::now() < deadline, "watcher did not initialize");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn saved_pom_burst_emits_one_debounced_event_with_updated_model() {
        let root = project_root("debounce");
        let pom = root.join("pom.xml");
        let mut watcher = MavenPomWatcher::start_with_timing(
            root.clone(),
            vec![pom.clone()],
            0,
            Duration::from_millis(10),
            Duration::from_millis(80),
        )
        .unwrap();
        wait_ready(&watcher);

        fs::write(
            &pom,
            "<project><modelVersion>4.0.0</modelVersion><name>first</name></project>",
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(25));
        fs::write(
            &pom,
            "<project><modelVersion>4.0.0</modelVersion><name>second</name></project>",
        )
        .unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        let event = loop {
            if let Some(event) = watcher.poll().into_iter().next() {
                break event;
            }
            assert!(
                Instant::now() < deadline,
                "watcher did not report the POM change"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(event.root, root);
        assert_eq!(event.project_generation, 0);
        assert!(event.model.is_ok());

        let quiet_deadline = Instant::now() + Duration::from_millis(140);
        while Instant::now() < quiet_deadline {
            assert!(
                watcher.poll().is_empty(),
                "one save burst emitted more than one event"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        watcher.cancel();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancel_stops_and_joins_the_watcher_worker() {
        let root = project_root("cancel");
        let mut watcher = MavenPomWatcher::start_with_timing(
            root.clone(),
            vec![root.join("pom.xml")],
            1,
            Duration::from_secs(10),
            Duration::from_millis(50),
        )
        .unwrap();
        wait_ready(&watcher);
        watcher.cancel();
        assert!(!watcher.has_worker());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn request_cancel_is_non_blocking_and_worker_can_be_reaped_later() {
        let root = project_root("async-cancel");
        let mut watcher = MavenPomWatcher::start_with_timing(
            root.clone(),
            vec![root.join("pom.xml")],
            0,
            Duration::from_millis(10),
            Duration::from_millis(80),
        )
        .unwrap();
        wait_ready(&watcher);
        watcher.request_cancel();
        let deadline = Instant::now() + Duration::from_secs(1);
        while !watcher.reap() {
            assert!(Instant::now() < deadline, "cancelled watcher did not stop");
            std::thread::sleep(Duration::from_millis(2));
        }
        fs::remove_dir_all(root).unwrap();
    }
}
