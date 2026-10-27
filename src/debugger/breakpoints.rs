use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};

#[derive(Clone, Debug)]
pub struct Breakpoint {
    pub source: PathBuf,
    pub line: u32,
    pub condition: Option<String>,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub enum BreakpointStoreEvent {
    Loaded {
        root: PathBuf,
        generation: u64,
        result: Result<Vec<Breakpoint>, String>,
    },
    Saved {
        root: PathBuf,
        generation: u64,
        result: Result<(), String>,
    },
}

enum BreakpointStoreCommand {
    Load {
        root: PathBuf,
        generation: u64,
    },
    Save {
        root: PathBuf,
        generation: u64,
        points: Vec<Breakpoint>,
    },
}

pub struct BreakpointStore {
    sender: mpsc::Sender<BreakpointStoreCommand>,
    receiver: mpsc::Receiver<BreakpointStoreEvent>,
    pending: Arc<AtomicUsize>,
    worker: Option<JoinHandle<()>>,
    startup_error: Option<String>,
}

impl Default for BreakpointStore {
    fn default() -> Self {
        let (sender, commands) = mpsc::channel();
        let (events, receiver) = mpsc::channel();
        let pending = Arc::new(AtomicUsize::new(0));
        let worker_pending = Arc::clone(&pending);
        let worker = thread::Builder::new()
            .name("breakpoint-store".into())
            .spawn(move || {
                while let Ok(command) = commands.recv() {
                    let event = match command {
                        BreakpointStoreCommand::Load { root, generation } => {
                            BreakpointStoreEvent::Loaded {
                                result: read_breakpoints(&root),
                                root,
                                generation,
                            }
                        }
                        BreakpointStoreCommand::Save {
                            root,
                            generation,
                            points,
                        } => BreakpointStoreEvent::Saved {
                            result: write_breakpoints(&root, &points),
                            root,
                            generation,
                        },
                    };
                    let _ = events.send(event);
                    worker_pending.fetch_sub(1, Ordering::AcqRel);
                }
            })
            .map_err(|error| error.to_string());
        let (worker, startup_error) = match worker {
            Ok(worker) => (Some(worker), None),
            Err(error) => (None, Some(error)),
        };
        Self {
            sender,
            receiver,
            pending,
            worker,
            startup_error,
        }
    }
}

impl BreakpointStore {
    pub fn load(&self, root: PathBuf, generation: u64) -> Result<(), String> {
        self.submit(BreakpointStoreCommand::Load { root, generation })
    }
    pub fn save(
        &self,
        root: PathBuf,
        generation: u64,
        points: Vec<Breakpoint>,
    ) -> Result<(), String> {
        self.submit(BreakpointStoreCommand::Save {
            root,
            generation,
            points,
        })
    }
    fn submit(&self, command: BreakpointStoreCommand) -> Result<(), String> {
        if let Some(error) = &self.startup_error {
            return Err(format!("Cannot start breakpoint storage: {error}"));
        }
        self.pending.fetch_add(1, Ordering::AcqRel);
        if self.sender.send(command).is_err() {
            self.pending.fetch_sub(1, Ordering::AcqRel);
            return Err("Breakpoint storage worker stopped unexpectedly".into());
        }
        Ok(())
    }
    pub fn poll(&mut self) -> Vec<BreakpointStoreEvent> {
        let events = self.receiver.try_iter().collect();
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
        events
    }
    pub fn has_pending(&self) -> bool {
        self.pending.load(Ordering::Acquire) > 0
    }
}

#[derive(Serialize, Deserialize)]
struct BreakpointFile {
    version: u32,
    breakpoints: Vec<StoredBreakpoint>,
}
#[derive(Serialize, Deserialize)]
struct StoredBreakpoint {
    source: PathBuf,
    line: u32,
    condition: Option<String>,
    enabled: bool,
}

fn read_breakpoints(root: &Path) -> Result<Vec<Breakpoint>, String> {
    let Some(path) = breakpoint_store_path(root, false)? else {
        return Ok(Vec::new());
    };
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    if !metadata.is_file() || metadata.len() > 4 * 1024 * 1024 {
        return Err(format!(
            "Invalid or oversized breakpoint store: {}",
            path.display()
        ));
    }
    let mut text = String::new();
    OpenOptions::new()
        .read(true)
        .open(&path)
        .and_then(|file| file.take(4 * 1024 * 1024 + 1).read_to_string(&mut text))
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let file: BreakpointFile =
        toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    if file.version != 1 {
        return Err(format!(
            "Unsupported breakpoint store version {}",
            file.version
        ));
    }
    let mut output = Vec::new();
    for point in file.breakpoints.into_iter().take(100_000) {
        if point.line == 0
            || point
                .condition
                .as_ref()
                .is_some_and(|condition| condition.len() > 4096 || condition.contains('\0'))
            || !safe_relative_path(&point.source)
        {
            continue;
        }
        output.push(Breakpoint {
            source: root.join(point.source),
            line: point.line,
            condition: point.condition,
            enabled: point.enabled,
        });
    }
    Ok(output)
}

fn write_breakpoints(root: &Path, points: &[Breakpoint]) -> Result<(), String> {
    static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
    let path = breakpoint_store_path(root, true)?.ok_or("Missing breakpoint storage path")?;
    let root = root
        .canonicalize()
        .map_err(|error| format!("{}: {error}", root.display()))?;
    let breakpoints = points
        .iter()
        .filter_map(|point| {
            let source = point.source.strip_prefix(&root).ok()?.to_path_buf();
            if point.line == 0 || !safe_relative_path(&source) {
                return None;
            }
            Some(StoredBreakpoint {
                source,
                line: point.line,
                condition: point.condition.clone(),
                enabled: point.enabled,
            })
        })
        .collect();
    let text = toml::to_string_pretty(&BreakpointFile {
        version: 1,
        breakpoints,
    })
    .map_err(|error| format!("Cannot encode breakpoints: {error}"))?;
    if text.len() > 4 * 1024 * 1024 {
        return Err("Breakpoint store exceeds 4 MiB".into());
    }
    let parent = path
        .parent()
        .ok_or("Missing breakpoint storage directory")?;
    let temporary = parent.join(format!(
        ".breakpoints-{}-{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        if breakpoint_store_path(&root, false)
            .map_err(|error| std::io::Error::other(error))?
            .as_ref()
            != Some(&path)
        {
            return Err(std::io::Error::other(
                "Breakpoint store changed during save",
            ));
        }
        fs::rename(&temporary, &path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|error| format!("{}: {error}", path.display()))
}

fn breakpoint_store_path(root: &Path, create: bool) -> Result<Option<PathBuf>, String> {
    let project = root
        .canonicalize()
        .map_err(|error| format!("Project {}: {error}", root.display()))?;
    if !project.is_dir() {
        return Err("Project root is not a directory".into());
    }
    let directory = project.join(".idea-rust");
    let metadata = match fs::symlink_metadata(&directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !create => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&directory)
                .map_err(|error| format!("{}: {error}", directory.display()))?;
            fs::symlink_metadata(&directory)
                .map_err(|error| format!("{}: {error}", directory.display()))?
        }
        Err(error) => return Err(format!("{}: {error}", directory.display())),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "Unsafe configuration directory {}",
            directory.display()
        ));
    }
    let canonical = directory
        .canonicalize()
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    if canonical != directory || !canonical.starts_with(&project) {
        return Err("Breakpoint directory escapes the project".into());
    }
    let path = canonical.join("breakpoints.toml");
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(format!("Unsafe breakpoint store {}", path.display()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("{}: {error}", path.display())),
    }
    Ok(Some(path))
}

fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}
#[derive(Clone, Debug)]
pub struct BreakpointStatus {
    pub id: Option<i64>,
    pub source: Option<PathBuf>,
    pub verified: bool,
    pub line: Option<u32>,
    pub message: Option<String>,
}
impl BreakpointStatus {
    pub fn from_adapter(value: &Value) -> Self {
        Self {
            id: value["id"].as_i64(),
            source: value["source"]["path"].as_str().map(PathBuf::from),
            verified: value["verified"].as_bool().unwrap_or(false),
            line: value["line"].as_u64().map(|line| line as u32),
            message: value["message"].as_str().map(str::to_owned),
        }
    }
    fn update(&mut self, reason: &str, value: &Value) {
        let updated = Self::from_adapter(value);
        if updated.id.is_some() {
            self.id = updated.id;
        }
        if updated.source.is_some() {
            self.source = updated.source;
        }
        if updated.line.is_some() {
            self.line = updated.line;
        }
        if reason == "removed" {
            self.verified = false;
        } else if value.get("verified").is_some() {
            self.verified = updated.verified;
        }
        if let Some(message) = updated.message {
            self.message = Some(message);
        }
    }
}
struct RetainedEvent {
    sequence: u64,
    key: Option<(PathBuf, u32)>,
    reason: String,
    breakpoint: Value,
}
struct InFlightRevision {
    revision: u64,
    events: Vec<RetainedEvent>,
}
#[derive(Default)]
pub struct Breakpoints {
    pub sources: BTreeMap<PathBuf, Vec<Breakpoint>>,
    pub status: BTreeMap<(PathBuf, u32), BreakpointStatus>,
    pub revision: u64,
    revisions: BTreeMap<PathBuf, u64>,
    pub modified: BTreeSet<PathBuf>,
    in_flight: BTreeMap<PathBuf, InFlightRevision>,
    event_sequence: u64,
}
impl Breakpoints {
    pub fn rename_source(&mut self, old: &Path, new: PathBuf) -> bool {
        if old == new { return false; }
        let Some(points) = self.sources.remove(old) else { return false; };
        self.status.retain(|(source, _), _| source != old && source != &new);
        self.modified.remove(old);
        self.modified.remove(&new);
        self.in_flight.remove(old);
        self.in_flight.remove(&new);
        let entries = self.sources.entry(new.clone()).or_default();
        for mut point in points {
            point.source = new.clone();
            if !entries.iter().any(|existing| existing.line == point.line) {
                entries.push(point);
            }
        }
        entries.sort_by_key(|point| point.line);
        self.revision = self.revision.wrapping_add(1);
        self.revisions.remove(old);
        self.revisions.insert(new.clone(), self.revision);
        self.modified.insert(new);
        true
    }

    pub fn snapshot(&self) -> Vec<Breakpoint> {
        self.sources.values().flatten().cloned().collect()
    }
    pub fn restore(&mut self, points: Vec<Breakpoint>, touched: &BTreeSet<PathBuf>) {
        let mut grouped = BTreeMap::<PathBuf, Vec<Breakpoint>>::new();
        for point in points {
            if touched.contains(&point.source) {
                continue;
            }
            let entries = grouped.entry(point.source.clone()).or_default();
            if !entries.iter().any(|existing| existing.line == point.line) {
                entries.push(point);
            }
        }
        for (source, mut entries) in grouped {
            entries.sort_by_key(|point| point.line);
            self.sources.insert(source, entries);
        }
        self.status.clear();
    }
    pub fn clear_adapter_state(&mut self) {
        self.status.clear();
        self.clear_retained_events();
    }
    pub fn clear_retained_events(&mut self) {
        self.in_flight.clear();
        self.event_sequence = 0;
    }
    pub fn begin_sync(&mut self, source: &Path, revision: u64) {
        if revision != self.source_revision(source) {
            return;
        }
        if self
            .in_flight
            .get(source)
            .is_some_and(|sync| sync.revision == revision)
        {
            return;
        }
        self.in_flight.insert(
            source.to_path_buf(),
            InFlightRevision {
                revision,
                events: Vec::new(),
            },
        );
    }
    pub fn finish_sync(&mut self, source: &Path, revision: u64) {
        if self
            .in_flight
            .get(source)
            .is_some_and(|sync| sync.revision == revision)
        {
            self.in_flight.remove(source);
        }
    }
    fn matching_key(&self, breakpoint: &Value) -> Option<(PathBuf, u32)> {
        if let Some(id) = breakpoint["id"].as_i64() {
            if let Some((key, _)) = self.status.iter().find(|(_, status)| status.id == Some(id)) {
                return Some(key.clone());
            }
        }
        let source = PathBuf::from(breakpoint["source"]["path"].as_str()?);
        let line = breakpoint["line"].as_u64()? as u32;
        if let Some((key, _)) = self.status.iter().find(|(_, status)| {
            status.source.as_ref() == Some(&source) && status.line == Some(line)
        }) {
            return Some(key.clone());
        }
        self.lines(&source)
            .into_iter()
            .find(|requested| {
                *requested == line
                    || self
                        .status
                        .get(&(source.clone(), *requested))
                        .is_some_and(|status| status.line == Some(line))
            })
            .map(|requested| (source, requested))
    }
    fn update_event(&mut self, key: (PathBuf, u32), reason: &str, breakpoint: &Value) {
        let status = self
            .status
            .entry(key)
            .or_insert_with(|| BreakpointStatus::from_adapter(breakpoint));
        status.update(reason, breakpoint);
    }
    pub fn adapter_event(&mut self, reason: &str, breakpoint: &Value) {
        let key = self.matching_key(breakpoint);
        let event_source = breakpoint["source"]["path"].as_str().map(PathBuf::from);
        let id = breakpoint["id"].as_i64();
        self.event_sequence = self.event_sequence.wrapping_add(1);
        // Keep matched notifications too: their newer state must survive an
        // older setBreakpoints response that has not reached the UI yet.
        for (source, sync) in &mut self.in_flight {
            let matched = key.as_ref().is_some_and(|(path, _)| path == source);
            let unknown_id = key.is_none() && id.is_some();
            if matched || event_source.as_ref() == Some(source) || unknown_id {
                sync.events.push(RetainedEvent {
                    sequence: self.event_sequence,
                    key: key.as_ref().filter(|(path, _)| path == source).cloned(),
                    reason: reason.into(),
                    breakpoint: breakpoint.clone(),
                });
                if sync.events.len() > 64 {
                    sync.events.remove(0);
                }
            }
        }
        // ID-only events can precede the response's ID/source mapping. Bound
        // their total retention even with many simultaneous source requests.
        while self
            .in_flight
            .values()
            .map(|sync| sync.events.len())
            .sum::<usize>()
            > 1024
        {
            let oldest = self
                .in_flight
                .iter()
                .filter_map(|(source, sync)| {
                    sync.events
                        .first()
                        .map(|event| (event.sequence, source.clone()))
                })
                .min_by_key(|(sequence, _)| *sequence)
                .map(|(_, source)| source);
            if let Some(source) = oldest {
                if let Some(sync) = self.in_flight.get_mut(&source) {
                    sync.events.remove(0);
                }
            } else {
                break;
            }
        }
        if let Some(key) = key {
            self.update_event(key, reason, breakpoint);
        }
    }
    pub fn toggle(&mut self, source: PathBuf, line: u32) {
        if line == 0 {
            return;
        }
        let entries = self.sources.entry(source.clone()).or_default();
        if let Some(index) = entries.iter().position(|point| point.line == line) {
            entries.remove(index);
        } else {
            entries.push(Breakpoint {
                source: source.clone(),
                line,
                condition: None,
                enabled: true,
            });
            entries.sort_by_key(|point| point.line);
        }
        self.status.retain(|(path, _), _| path != &source);
        self.in_flight.remove(&source);
        self.revision = self.revision.wrapping_add(1);
        self.revisions.insert(source, self.revision);
    }
    pub fn update(
        &mut self,
        source: &Path,
        line: u32,
        condition: Option<String>,
        enabled: bool,
    ) -> bool {
        let Some(point) = self
            .sources
            .get_mut(source)
            .and_then(|points| points.iter_mut().find(|point| point.line == line))
        else {
            return false;
        };
        if point.condition == condition && point.enabled == enabled {
            return false;
        }
        point.condition = condition;
        point.enabled = enabled;
        self.status.remove(&(source.to_path_buf(), line));
        self.in_flight.remove(source);
        self.revision = self.revision.wrapping_add(1);
        self.revisions.insert(source.to_path_buf(), self.revision);
        true
    }
    pub fn source_revision(&self, source: &Path) -> u64 {
        self.revisions.get(source).copied().unwrap_or(0)
    }
    pub fn lines(&self, source: &Path) -> Vec<u32> {
        self.sources
            .get(source)
            .into_iter()
            .flatten()
            .filter(|point| point.enabled)
            .map(|point| point.line)
            .collect()
    }
    pub fn arguments(&self, source: &Path) -> Value {
        let points: Vec<_> = self
            .sources
            .get(source)
            .into_iter()
            .flatten()
            .filter(|point| point.enabled)
            .map(|point| json!({"line":point.line,"condition":point.condition}))
            .collect();
        json!({"source":{"path":source,"name":source.file_name().map(|s|s.to_string_lossy())},"breakpoints":points,"sourceModified":self.modified.contains(source)})
    }
    pub fn initial_requests(&self) -> Vec<(String, Value)> {
        self.sources
            .keys()
            .map(|source| ("setBreakpoints".into(), self.arguments(source)))
            .collect()
    }
    pub fn apply(&mut self, source: &Path, revision: u64, statuses: Vec<BreakpointStatus>) {
        if revision != self.source_revision(source) {
            return;
        }
        let retained = self
            .in_flight
            .remove(source)
            .filter(|sync| sync.revision == revision)
            .map(|sync| sync.events)
            .unwrap_or_default();
        self.status.retain(|(path, _), _| path != source);
        for (line, status) in self.lines(source).into_iter().zip(statuses) {
            self.status.insert((source.to_path_buf(), line), status);
        }
        // Response establishes the ID mapping, then notifications replay in
        // arrival order using the stable requested line or the response's ID.
        for event in retained {
            let key = event.key.or_else(|| self.matching_key(&event.breakpoint));
            if let Some(key) = key.filter(|(path, _)| path == source) {
                self.update_event(key, &event.reason, &event.breakpoint);
            }
        }
    }
}
