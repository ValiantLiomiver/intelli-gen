use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant, SystemTime};

use super::{ProjectModel, maven_cache::fingerprint_pom_paths};

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
    let _ = super::maven::profile_pom_paths(&root, &mut pom_paths);
    pom_paths.sort();
    pom_paths.dedup();
    let mut previous = metadata_snapshot(&pom_paths).ok();
    let mut previous_content = content_snapshot(&root, &pom_paths);
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
            let fingerprint = content_snapshot(&root, &pom_paths);
            if previous_content != fingerprint {
                previous_content = fingerprint;
                changed_at = Some(Instant::now());
            }
        }
        if changed_at.is_some_and(|changed| changed.elapsed() >= debounce) {
            let model = ProjectModel::discover(&root).map_err(|error| error.to_string());
            if let Ok(model) = &model {
                let mut discovered = project_pom_paths(model);
                if super::maven::profile_pom_paths(&root, &mut discovered).is_ok() {
                    pom_paths = discovered;
                } else {
                    pom_paths.extend(discovered);
                }
                pom_paths.sort();
                pom_paths.dedup();
                previous = metadata_snapshot(&pom_paths).ok();
                previous_content = content_snapshot(&root, &pom_paths);
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

fn content_snapshot(
    root: &std::path::Path,
    paths: &[PathBuf],
) -> Vec<(PathBuf, Result<String, String>)> {
    let mut paths = paths.to_vec();
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .map(|path| {
            let result = fingerprint_pom_paths(root, std::slice::from_ref(&path));
            (path, result)
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PomMetadata {
    path: PathBuf,
    modified: Option<SystemTime>,
    length: Option<u64>,
    error: Option<String>,
}

fn metadata_snapshot(paths: &[PathBuf]) -> Result<Vec<PomMetadata>, String> {
    let mut paths = paths.to_vec();
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .map(|path| match fs::metadata(&path) {
            Ok(metadata) => {
                let modified = metadata.modified();
                Ok(PomMetadata {
                    path,
                    modified: modified.as_ref().ok().copied(),
                    length: Some(metadata.len()),
                    error: modified.err().map(|error| error.to_string()),
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(PomMetadata {
                path,
                modified: None,
                length: None,
                error: None,
            }),
            Err(error) => Ok(PomMetadata {
                path,
                modified: None,
                length: None,
                error: Some(error.to_string()),
            }),
        })
        .collect()
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

    #[test]
    fn inaccessible_pom_does_not_prevent_observing_other_paths() {
        let root = project_root("metadata-error");
        let obstacle = root.join("not-a-directory");
        fs::write(&obstacle, "file").unwrap();
        let paths = vec![root.join("pom.xml"), obstacle.join("pom.xml")];
        let snapshot = super::metadata_snapshot(&paths).unwrap();
        assert_eq!(snapshot.len(), 2);
        assert!(
            snapshot
                .iter()
                .any(|item| item.path == root.join("pom.xml") && item.length.is_some())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn content_check_continues_when_one_pom_is_unreadable() {
        let root = project_root("content-error");
        let obstacle = root.join("not-a-directory");
        fs::write(&obstacle, "file").unwrap();
        let paths = vec![root.join("pom.xml"), obstacle.join("pom.xml")];
        let before = super::content_snapshot(&root, &paths);
        fs::write(
            root.join("pom.xml"),
            "<project><name>changed</name></project>",
        )
        .unwrap();
        assert_ne!(before, super::content_snapshot(&root, &paths));
        fs::remove_dir_all(root).unwrap();
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
    fn profile_module_pom_change_triggers_reload() {
        let root = project_root("profile-module");
        let module = root.join("extra");
        fs::create_dir_all(&module).unwrap();
        fs::write(root.join("pom.xml"), "<project><profiles><profile><id>extra</id><modules><module>extra</module></modules></profile></profiles></project>").unwrap();
        let pom = module.join("pom.xml");
        fs::write(&pom, "<project><name>before</name></project>").unwrap();
        let mut watcher = MavenPomWatcher::start_with_timing(
            root.clone(),
            vec![root.join("pom.xml")],
            1,
            Duration::from_millis(10),
            Duration::from_millis(30),
        )
        .unwrap();
        wait_ready(&watcher);
        fs::write(&pom, "<project><name>after-longer</name></project>").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while watcher.poll().is_empty() {
            assert!(
                Instant::now() < deadline,
                "profile module POM change was not detected"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        watcher.cancel();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repairing_a_malformed_profile_module_triggers_reload() {
        let root = project_root("profile-invalid");
        let module = root.join("extra");
        fs::create_dir_all(&module).unwrap();
        fs::write(root.join("pom.xml"), "<project><profiles><profile><modules><module>extra</module></modules></profile></profiles></project>").unwrap();
        let pom = module.join("pom.xml");
        fs::write(&pom, "<project>").unwrap();
        let mut watcher = MavenPomWatcher::start_with_timing(
            root.clone(),
            vec![root.join("pom.xml")],
            1,
            Duration::from_millis(10),
            Duration::from_millis(30),
        )
        .unwrap();
        wait_ready(&watcher);
        fs::write(&pom, "<project><name>repaired</name></project>").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while watcher.poll().is_empty() {
            assert!(
                Instant::now() < deadline,
                "repair of profile module POM was not detected"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        watcher.cancel();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn removed_profile_module_does_not_block_future_root_changes() {
        let root = project_root("profile-removed");
        let module = root.join("extra");
        fs::create_dir_all(&module).unwrap();
        fs::write(module.join("pom.xml"), "<project/>").unwrap();
        let pom = root.join("pom.xml");
        fs::write(&pom, "<project><profiles><profile><modules><module>extra</module></modules></profile></profiles></project>").unwrap();
        let mut watcher = MavenPomWatcher::start_with_timing(
            root.clone(),
            vec![pom.clone()],
            1,
            Duration::from_millis(10),
            Duration::from_millis(30),
        )
        .unwrap();
        wait_ready(&watcher);
        fs::write(&pom, "<project><profiles><profile><modules><module>${unknown}</module></modules></profile></profiles></project>").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while watcher.poll().is_empty() {
            assert!(Instant::now() < deadline, "module removal not detected");
            std::thread::sleep(Duration::from_millis(5));
        }
        fs::remove_dir_all(&module).unwrap();
        fs::write(&module, "not a directory").unwrap();
        fs::write(&pom, "<project><name>another change</name></project>").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while watcher.poll().is_empty() {
            assert!(
                Instant::now() < deadline,
                "stale module prevented subsequent reload"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        watcher.cancel();
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
