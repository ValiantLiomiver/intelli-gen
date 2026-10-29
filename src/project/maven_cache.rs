use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use super::ProjectModel;
use super::maven_view::MavenView;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedMavenView {
    root: PathBuf,
    pub view: MavenView,
    pub pom_fingerprint: String,
}

pub struct MavenViewCache {
    directory: PathBuf,
}

#[derive(Debug)]
pub struct MavenCacheEvent {
    pub root: PathBuf,
    pub generation: u64,
    pub result: Result<Option<CachedMavenView>, String>,
    pub current_pom_fingerprint: Result<String, String>,
}

pub struct MavenCacheLoader {
    sender: Sender<MavenCacheEvent>,
    receiver: Receiver<MavenCacheEvent>,
    latest: HashMap<PathBuf, u64>,
    workers: Vec<(PathBuf, JoinHandle<()>)>,
}

impl Default for MavenCacheLoader {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            sender,
            receiver,
            latest: HashMap::new(),
            workers: Vec::new(),
        }
    }
}

impl MavenCacheLoader {
    pub fn start(&mut self, root: PathBuf, generation: u64) -> Result<(), String> {
        if self
            .latest
            .get(&root)
            .is_some_and(|latest| *latest >= generation)
        {
            return Err("Maven cache generation must increase for each request".into());
        }
        let sender = self.sender.clone();
        let worker_root = root.clone();
        let worker = thread::Builder::new()
            .name("maven-cache".into())
            .spawn(move || {
                let current_pom_fingerprint = ProjectModel::discover(&worker_root)
                    .map_err(|error| error.to_string())
                    .and_then(|model| fingerprint_poms(&model));
                let result = MavenViewCache::for_user().and_then(|cache| cache.load(&worker_root));
                let _ = sender.send(MavenCacheEvent {
                    root: worker_root,
                    generation,
                    result,
                    current_pom_fingerprint,
                });
            })
            .map_err(|error| format!("Cannot start Maven cache worker: {error}"))?;
        self.latest.insert(root.clone(), generation);
        self.workers.push((root, worker));
        Ok(())
    }

    pub fn cancel(&mut self) {
        self.latest.clear();
    }

    pub fn poll(&mut self) -> Vec<MavenCacheEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.receiver.try_recv() {
            if self.latest.get(&event.root) == Some(&event.generation) {
                events.push(event);
            }
        }
        let mut index = 0;
        while index < self.workers.len() {
            if self.workers[index].1.is_finished() {
                let _ = self.workers.swap_remove(index).1.join();
            } else {
                index += 1;
            }
        }
        events
    }

    pub fn has_workers(&self) -> bool {
        !self.workers.is_empty()
    }
}

impl MavenViewCache {
    pub fn for_user() -> Result<Self, String> {
        let directories = ProjectDirs::from("dev", "Intelli-gen", "Intelli-gen")
            .ok_or_else(|| "Maven cache directory is unavailable".to_owned())?;
        Ok(Self::at(directories.cache_dir().join("maven-views")))
    }

    pub fn at(directory: PathBuf) -> Self {
        Self { directory }
    }

    pub fn load(&self, root: &Path) -> Result<Option<CachedMavenView>, String> {
        let canonical_root = root
            .canonicalize()
            .map_err(|error| format!("Cannot resolve Maven cache project root: {error}"))?;
        let path = self.path_for(&canonical_root);
        let data = match fs::read(&path) {
            Ok(data) => data,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(format!(
                    "Cannot read Maven cache {}: {error}",
                    path.display()
                ));
            }
        };
        let cached: CachedMavenView = serde_json::from_slice(&data)
            .map_err(|error| format!("Cannot parse Maven cache {}: {error}", path.display()))?;
        if cached.root != canonical_root {
            return Err(format!(
                "Maven cache {} belongs to another project",
                path.display()
            ));
        }
        Ok(Some(cached))
    }

    pub fn save(&self, root: &Path, view: &MavenView, pom_fingerprint: &str) -> Result<(), String> {
        let canonical_root = root
            .canonicalize()
            .map_err(|error| format!("Cannot resolve Maven cache project root: {error}"))?;
        let path = self.path_for(&canonical_root);
        let parent = path
            .parent()
            .ok_or_else(|| "Maven cache path has no parent directory".to_owned())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("Cannot create Maven cache directory: {error}"))?;

        let cached = CachedMavenView {
            root: canonical_root,
            view: view.clone(),
            pom_fingerprint: pom_fingerprint.to_owned(),
        };
        let data = serde_json::to_vec(&cached)
            .map_err(|error| format!("Cannot serialize Maven cache: {error}"))?;
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(".maven-view-{}-{sequence}.tmp", std::process::id()));
        let result = (|| -> io::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&data)?;
            file.sync_all()?;
            fs::rename(&temporary, &path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(|error| format!("Cannot write Maven cache {}: {error}", path.display()))
    }

    fn path_for(&self, root: &Path) -> PathBuf {
        let mut hasher = DefaultHasher::new();
        root.hash(&mut hasher);
        self.directory
            .join(format!("{:016x}.json", hasher.finish()))
    }
}

pub fn fingerprint_poms(model: &ProjectModel) -> Result<String, String> {
    let mut paths = vec![model.root.join("pom.xml")];
    paths.extend(
        model
            .modules
            .iter()
            .map(|module| module.root.join("pom.xml")),
    );
    fingerprint_pom_paths(&model.root, &paths)
}

pub fn fingerprint_pom_paths(root: &Path, paths: &[PathBuf]) -> Result<String, String> {
    let mut paths = paths.to_vec();
    paths.sort();
    paths.dedup();

    let mut hasher = DefaultHasher::new();
    for path in paths {
        path.strip_prefix(root)
            .unwrap_or(path.as_path())
            .hash(&mut hasher);
        match fs::read(&path) {
            Ok(contents) => contents.hash(&mut hasher),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                "<missing-pom>".hash(&mut hasher);
            }
            Err(error) => {
                return Err(format!(
                    "Cannot fingerprint Maven POM {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Ok(format!("{:016x}", hasher.finish()))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{MavenCacheLoader, fingerprint_poms};
    use crate::project::maven_view::{MavenModuleView, MavenProfile, MavenTask, MavenView};
    use crate::project::{ProjectKind, ProjectModel, ProjectModule};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn temp_root(label: &str) -> PathBuf {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "intelli-gen-maven-cache-test-{}-{label}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn sample_view(name: &str) -> MavenView {
        MavenView {
            profiles: vec![MavenProfile {
                id: "dev".into(),
                active_by_default: false,
            }],
            root_module: MavenModuleView {
                name: name.into(),
                relative_path: None,
                lifecycle: vec![MavenTask {
                    label: "Package".into(),
                    goal: "package".into(),
                }],
                plugins: Vec::new(),
                dependencies: vec!["org.example:lib:1.0".into()],
                repositories: Vec::new(),
                modules: Vec::new(),
            },
        }
    }

    fn model(root: &Path, modules: Vec<ProjectModule>) -> ProjectModel {
        ProjectModel {
            root: root.to_path_buf(),
            kind: ProjectKind::Maven,
            name: root
                .file_name()
                .unwrap_or(root.as_os_str())
                .to_string_lossy()
                .into_owned(),
            modules,
            wrapper: None,
        }
    }

    #[test]
    fn cache_round_trips_the_maven_view_and_fingerprint() {
        let root = temp_root("round-trip");
        let cache = super::MavenViewCache::at(root.join("cache"));
        let view = sample_view("cached-project");
        cache.save(&root, &view, "fingerprint-1").unwrap();

        let cached = cache.load(&root).unwrap().unwrap();
        assert_eq!(cached.pom_fingerprint, "fingerprint-1");
        assert_eq!(
            serde_json::to_value(cached.view).unwrap(),
            serde_json::to_value(view).unwrap()
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cache_isolated_by_project_root() {
        let first = temp_root("first");
        let second = temp_root("second");
        let cache = super::MavenViewCache::at(first.join("cache"));
        cache.save(&first, &sample_view("first"), "one").unwrap();

        assert!(cache.load(&second).unwrap().is_none());

        fs::remove_dir_all(first).unwrap();
        fs::remove_dir_all(second).unwrap();
    }

    #[test]
    fn missing_cache_returns_none() {
        let root = temp_root("missing");
        let cache = super::MavenViewCache::at(root.join("cache"));
        assert!(cache.load(&root).unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_cache_returns_a_readable_error() {
        let root = temp_root("corrupt");
        let cache = super::MavenViewCache::at(root.join("cache"));
        fs::create_dir_all(root.join("cache")).unwrap();
        let canonical_root = root.canonicalize().unwrap();
        fs::write(cache.path_for(&canonical_root), b"not valid cache data").unwrap();

        let error = cache.load(&root).unwrap_err();
        assert!(error.contains("cache"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pom_fingerprint_is_stable_and_changes_with_any_reactor_pom() {
        let root = temp_root("fingerprint");
        let module = root.join("module-a");
        fs::create_dir_all(&module).unwrap();
        fs::write(root.join("pom.xml"), "<project>root</project>").unwrap();
        fs::write(module.join("pom.xml"), "<project>module</project>").unwrap();
        let project = model(
            &root,
            vec![ProjectModule {
                name: "module-a".into(),
                root: module.clone(),
            }],
        );

        let first = fingerprint_poms(&project).unwrap();
        assert_eq!(first, fingerprint_poms(&project).unwrap());
        fs::write(module.join("pom.xml"), "<project>changed</project>").unwrap();
        assert_ne!(first, fingerprint_poms(&project).unwrap());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cache_loader_reads_off_worker_and_returns_root_generation() {
        let root = temp_root("async-load");
        let root = root.canonicalize().unwrap();
        let mut loader = MavenCacheLoader::default();
        loader.start(root.clone(), 7).unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let event = loop {
            if let Some(event) = loader.poll().into_iter().next() {
                break event;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "cache read worker timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(event.root, root);
        assert_eq!(event.generation, 7);
        assert!(event.result.unwrap().is_none());

        fs::remove_dir_all(root).unwrap();
    }
}
