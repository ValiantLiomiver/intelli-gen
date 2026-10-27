//! Project-local run configurations and cancellable direct Java launches.
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc as std_mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::{mpsc, watch};

use super::jdk::JdkInstallation;
use super::lsp::ClasspathResolver;
use crate::project::ProjectModel;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunConfiguration {
    pub id: String,
    pub name: String,
    pub main_class: String,
    /// JPMS module passed to `--module`; independent of the Maven/Gradle project module.
    #[serde(default)] pub module: Option<String>,
    /// Module path relative to the build root. `None` detects the module from the main class.
    #[serde(default)] pub project_module: Option<PathBuf>,
    #[serde(default)] pub program_args: Vec<String>,
    #[serde(default)] pub vm_args: Vec<String>,
    pub working_directory: PathBuf,
    #[serde(default)] pub environment: BTreeMap<String, String>,
    #[serde(default)] pub jdk_id: Option<String>,
}
impl RunConfiguration {
    pub fn new(root: &Path) -> Self {
        Self { id: fresh_id(), name: "Application".into(), main_class: String::new(), module: None, project_module: None,
            program_args: Vec::new(), vm_args: Vec::new(), working_directory: root.to_path_buf(),
            environment: BTreeMap::new(), jdk_id: None }
    }
    pub fn validate(&self) -> Result<(), RunError> {
        if self.id.trim().is_empty() || self.name.trim().is_empty() { return Err(RunError("Configuration ID and name are required".into())); }
        if self.main_class.trim().is_empty() || self.main_class.starts_with('-') || self.main_class.contains(['/', '\\', '\0']) {
            return Err(RunError("Enter a Java main class such as com.example.Main".into()));
        }
        if self.module.as_ref().is_some_and(|module| module.is_empty() || module.starts_with('-') || module.contains(['/', '\\', '\0'])) {
            return Err(RunError("Module must be a Java module name".into()));
        }
        if self.project_module.as_ref().is_some_and(|path| path.is_absolute() || path.components().any(|part| matches!(part, std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_)))) {
            return Err(RunError("Project module must be a path relative to the build root".into()));
        }
        if self.program_args.iter().chain(&self.vm_args).any(|arg| arg.contains('\0')) {
            return Err(RunError("Arguments cannot contain NUL characters".into()));
        }
        if self.environment.iter().any(|(key, value)| key.is_empty() || key.contains(['=', '\0']) || value.contains('\0')) {
            return Err(RunError("Environment keys must be nonempty and cannot contain = or NUL".into()));
        }
        Ok(())
    }
    pub fn working_directory_at(&self, root: &Path) -> PathBuf {
        if self.working_directory.is_absolute() { self.working_directory.clone() }
        else { root.join(&self.working_directory) }
    }
}
fn fresh_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!("run-{}-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}
#[derive(Debug)]
pub struct RunError(pub String);
impl fmt::Display for RunError { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.0) } }
impl std::error::Error for RunError {}
pub type RunConfigError = RunError;
#[derive(Default, Serialize, Deserialize)]
struct ConfigurationFile { #[serde(default)] configurations: Vec<RunConfiguration> }
pub struct RunConfigurationStore;
impl RunConfigurationStore {
    pub fn path(root: &Path) -> PathBuf { root.join(".idea-rust/run-configurations.toml") }
    /// Resolve only a real project-local directory. In particular, an existing
    /// `.idea-rust` symlink must not redirect reads or temporary writes elsewhere.
    fn checked_path(root: &Path, create: bool) -> Result<Option<PathBuf>, RunConfigError> {
        let project = root.canonicalize().map_err(|error| RunError(format!("Project {}: {error}", root.display())))?;
        if !project.is_dir() { return Err(RunError("Project root is not a directory".into())); }
        let directory = project.join(".idea-rust");
        let metadata = match fs::symlink_metadata(&directory) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !create { return Ok(None); }
                match fs::create_dir(&directory) {
                    Ok(()) => {},
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {},
                    Err(error) => return Err(RunError(format!("{}: {error}", directory.display()))),
                }
                fs::symlink_metadata(&directory).map_err(|error| RunError(format!("{}: {error}", directory.display())))?
            }
            Err(error) => return Err(RunError(format!("{}: {error}", directory.display()))),
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(RunError(format!("Unsafe configuration directory {}: expected a real directory, not a symlink", directory.display())));
        }
        let canonical = directory.canonicalize().map_err(|error| RunError(format!("{}: {error}", directory.display())))?;
        if !canonical.starts_with(&project) || canonical != directory {
            return Err(RunError(format!("Configuration directory escapes the project: {}", directory.display())));
        }
        let path = canonical.join("run-configurations.toml");
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(RunError(format!("Unsafe run configuration store: {}", path.display())));
                }
                let target = path.canonicalize().map_err(|error| RunError(format!("{}: {error}", path.display())))?;
                if !target.starts_with(&project) || target.parent() != Some(canonical.as_path()) {
                    return Err(RunError(format!("Run configuration store escapes the project: {}", path.display())));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => return Err(RunError(format!("{}: {error}", path.display()))),
        }
        Ok(Some(path))
    }
    pub fn load(root: &Path) -> Result<Vec<RunConfiguration>, RunConfigError> {
        let Some(path) = Self::checked_path(root, false)? else { return Ok(Vec::new()); };
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(RunError(format!("{}: {error}", path.display()))),
        };
        let file: ConfigurationFile = toml::from_str(&text).map_err(|error| RunError(format!("{}: {error}", path.display())))?;
        let mut ids = HashSet::new();
        for config in &file.configurations {
            config.validate()?;
            if !ids.insert(&config.id) { return Err(RunError("Duplicate run configuration ID".into())); }
        }
        Ok(file.configurations)
    }
    pub fn save(root: &Path, configurations: &[RunConfiguration]) -> Result<(), RunConfigError> {
        let mut ids = HashSet::new();
        for config in configurations {
            config.validate()?;
            if !ids.insert(&config.id) { return Err(RunError("Duplicate run configuration ID".into())); }
        }
        let text = toml::to_string_pretty(&ConfigurationFile { configurations: configurations.to_vec() }).map_err(|error| RunError(error.to_string()))?;
        let path = Self::checked_path(root, true)?.ok_or_else(|| RunError("Missing configuration directory".into()))?;
        let parent = path.parent().ok_or_else(|| RunError("Missing configuration directory".into()))?;
        let temporary = parent.join(format!(".{}.tmp", fresh_id()));
        let result = (|| -> std::io::Result<()> {
            let mut file = OpenOptions::new().create_new(true).write(true).open(&temporary)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            // Recheck immediately before replacing the store, as well as before
            // creating the sibling file. Never follow a configuration symlink.
            let checked = Self::checked_path(root, false).map_err(|error| std::io::Error::other(error.to_string()))?;
            if checked.as_ref() != Some(&path) { return Err(std::io::Error::other("Configuration directory changed during save")); }
            fs::rename(&temporary, &path)
        })();
        if result.is_err() { let _ = fs::remove_file(&temporary); }
        result.map_err(|error| RunError(format!("{}: {error}", path.display())))
    }
}

#[derive(Clone, Debug)]
pub struct RunConfigurationLoadEvent {
    pub root: PathBuf,
    pub generation: u64,
    pub saved: bool,
    pub selected: Option<String>,
    pub result: Result<Vec<RunConfiguration>, String>,
}

enum RunConfigurationCommand {
    Load { root: PathBuf, generation: u64 },
    Save { root: PathBuf, generation: u64, configurations: Vec<RunConfiguration>, selected: Option<String> },
}

pub struct RunConfigurationLoader {
    channel: Option<(std_mpsc::Sender<RunConfigurationCommand>, std_mpsc::Receiver<RunConfigurationLoadEvent>)>,
    generation: u64,
    worker: Option<JoinHandle<()>>,
    pending: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    shutting_down: bool,
}

impl Default for RunConfigurationLoader {
    fn default() -> Self {
        Self { channel: None, generation: 0, worker: None,
            pending: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)), shutting_down: false }
    }
}

impl RunConfigurationLoader {
    fn start_worker(&mut self) -> Result<(), String> {
        if self.shutting_down { return Err("Run-configuration service is shutting down".into()); }
        if self.worker.is_some() { return Ok(()); }
        let (command_tx, command_rx) = std_mpsc::channel();
        let (event_tx, event_rx) = std_mpsc::channel();
        let pending = std::sync::Arc::clone(&self.pending);
        let worker = std::thread::Builder::new().name("run-config-io".into()).spawn(move || {
            while let Ok(command) = command_rx.recv() {
                let (root, generation, saved, selected, result) = match command {
                    RunConfigurationCommand::Load { root, generation } => {
                        let result = RunConfigurationStore::load(&root).map_err(|error| error.to_string());
                        let selected = result.as_ref().ok().and_then(|configs| configs.first().map(|config| config.id.clone()));
                        (root, generation, false, selected, result)
                    }
                    RunConfigurationCommand::Save { root, generation, configurations, selected } => {
                        let result = RunConfigurationStore::save(&root, &configurations)
                            .map(|()| configurations).map_err(|error| error.to_string());
                        (root, generation, true, selected, result)
                    }
                };
                let _ = event_tx.send(RunConfigurationLoadEvent { root, generation, saved, selected, result });
                pending.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
            }
        }).map_err(|error| format!("Cannot start run-configuration worker: {error}"))?;
        self.channel = Some((command_tx, event_rx));
        self.worker = Some(worker);
        Ok(())
    }

    fn submit(&mut self, command: RunConfigurationCommand, generation: u64) -> Result<(), String> {
        if generation <= self.generation { return Err("Run-configuration generation must increase".into()); }
        self.start_worker()?;
        let sender = self.channel.as_ref().expect("worker channel initialized").0.clone();
        self.pending.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        if sender.send(command).is_err() {
            self.pending.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
            return Err("Run-configuration worker stopped unexpectedly".into());
        }
        self.generation = generation;
        Ok(())
    }

    pub fn start(&mut self, root: PathBuf, generation: u64) -> Result<(), String> {
        self.submit(RunConfigurationCommand::Load { root, generation }, generation)
    }

    pub fn save(&mut self, root: PathBuf, generation: u64, configurations: Vec<RunConfiguration>, selected: Option<String>) -> Result<(), String> {
        self.submit(RunConfigurationCommand::Save { root, generation, configurations, selected }, generation)
    }

    pub fn poll(&mut self) -> Vec<RunConfigurationLoadEvent> {
        let events: Vec<_> = self.channel.as_ref().into_iter()
            .flat_map(|(_, receiver)| receiver.try_iter())
            .filter(|event| event.generation == self.generation)
            .collect();
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(worker) = self.worker.take() { let _ = worker.join(); }
        }
        events
    }

    pub fn shutdown(&mut self) {
        self.shutting_down = true;
        self.channel.take();
    }

    pub fn has_workers(&self) -> bool {
        self.pending.load(std::sync::atomic::Ordering::Acquire) > 0
            || (self.shutting_down && self.worker.as_ref().is_some_and(|worker| !worker.is_finished()))
    }
}

#[derive(Debug)]
pub enum RunEvent { Output { stderr: bool, text: String }, State(String), Exit { code: Option<i32>, cancelled: bool } }
pub struct RunHandle {
    events: mpsc::Receiver<RunEvent>,
    cancellation: watch::Sender<bool>,
    worker: Option<JoinHandle<()>>,
}
impl RunHandle {
    pub fn cancel(&self) { let _ = self.cancellation.send(true); }
    pub fn poll(&mut self) -> Vec<RunEvent> {
        let mut result = Vec::new();
        for _ in 0..128 { match self.events.try_recv() { Ok(event) => result.push(event), Err(_) => break } }
        result
    }
    fn reap(&mut self) -> Option<Result<(), RunError>> {
        if !self.worker.as_ref().is_some_and(|worker| worker.is_finished()) { return None; }
        Some(self.worker.take().unwrap().join().map_err(|_| RunError("Run worker panicked".into())))
    }
}
impl Drop for RunHandle { fn drop(&mut self) { self.cancel(); } }

pub struct RunService { resolver: ClasspathResolver }
impl RunService {
    pub fn new(resolver: ClasspathResolver) -> Self { Self { resolver } }
    pub fn start(&self, config: &RunConfiguration, project: &ProjectModel, jdk: &JdkInstallation) -> Result<RunHandle, RunError> {
        config.validate()?;
        let cwd = config.working_directory_at(&project.root);
        let executable = jdk.home.join("bin/java");
        let (events, receiver) = mpsc::channel(128);
        let (cancellation, cancel) = watch::channel(false);
        let config = config.clone();
        let project = project.clone();
        let jdk = jdk.clone();
        let resolver = self.resolver.clone();
        let worker = std::thread::Builder::new().name("java-run".into()).spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build();
            match runtime {
                Ok(runtime) => runtime.block_on(async {
                    let setup = jdk.child_environment().map_err(|error| RunError(error.to_string()))
                        .and_then(|environment| {
                            if cwd.is_dir() { Ok(environment) }
                            else { Err(RunError(format!("Working directory does not exist: {}", cwd.display()))) }
                        });
                    match setup {
                        Ok(environment) => if let Err(error) = launch(config, project, executable, environment, cwd, resolver, events.clone(), cancel).await {
                            let _ = events.send(RunEvent::State(format!("Run failed: {error}"))).await;
                        },
                        Err(error) => { let _ = events.send(RunEvent::State(format!("Run failed: {error}"))).await; }
                    }
                }),
                Err(error) => { let _ = events.blocking_send(RunEvent::State(format!("Run runtime failed: {error}"))); }
            }
        }).map_err(|error| RunError(error.to_string()))?;
        Ok(RunHandle { events: receiver, cancellation, worker: Some(worker) })
    }
}
async fn launch(config: RunConfiguration, project: ProjectModel, executable: PathBuf,
    environment: Vec<(std::ffi::OsString, std::ffi::OsString)>, cwd: PathBuf, resolver: ClasspathResolver,
    events: mpsc::Sender<RunEvent>, mut cancel: watch::Receiver<bool>) -> Result<(), RunError> {
    if *cancel.borrow() { return Ok(()); }
    let _ = events.send(RunEvent::State("Resolving runtime classpath with JDT LS…".into())).await;
    let paths = tokio::select! {
        _ = cancel.changed() => {
            let _ = events.send(RunEvent::State("Stopped before launch".into())).await;
            return Ok(());
        },
        result = resolver.resolve(&project, &config.main_class, config.project_module.as_deref()) => result.map_err(|error| RunError(error.to_string()))?,
    };
    if *cancel.borrow() { return Ok(()); }
    let mut command = tokio::process::Command::new(&executable);
    command.args(&config.vm_args);
    if !paths.classpaths.is_empty() {
        let joined = std::env::join_paths(&paths.classpaths).map_err(|error| RunError(error.to_string()))?;
        command.arg("-classpath").arg(joined);
    }
    if !paths.modulepaths.is_empty() {
        let joined = std::env::join_paths(&paths.modulepaths).map_err(|error| RunError(error.to_string()))?;
        command.arg("--module-path").arg(joined);
    }
    if let Some(module) = &config.module { command.arg("--module").arg(format!("{module}/{}", config.main_class)); }
    else { command.arg(&config.main_class); }
    // Each saved vector entry is one argv element. No shell or command parsing.
    command.args(&config.program_args).current_dir(cwd).envs(environment).envs(&config.environment)
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(unix)] command.process_group(0);
    let mut child = command.spawn().map_err(|error| RunError(format!("Cannot launch {}: {error}", executable.display())))?;
    let pid = child.id();
    let stdout = child.stdout.take().ok_or_else(|| RunError("Missing Java stdout".into()))?;
    let stderr = child.stderr.take().ok_or_else(|| RunError("Missing Java stderr".into()))?;
    let out = tokio::spawn(capture(stdout, false, events.clone()));
    let err = tokio::spawn(capture(stderr, true, events.clone()));
    let _ = events.send(RunEvent::State(format!("Running {}", config.name))).await;
    let (status, cancelled) = tokio::select! {
        result = child.wait() => (result, false),
        _ = cancel.changed() => {
            kill_group(pid);
            let _ = child.start_kill();
            (child.wait().await, true)
        }
    };
    kill_group(pid);
    for mut task in [out, err] {
        if tokio::time::timeout(Duration::from_millis(300), &mut task).await.is_err() { task.abort(); let _ = task.await; }
    }
    let status = status.map_err(|error| RunError(error.to_string()))?;
    let _ = events.send(RunEvent::Exit { code: status.code(), cancelled }).await;
    Ok(())
}
async fn capture(mut pipe: impl AsyncRead + Unpin, stderr: bool, events: mpsc::Sender<RunEvent>) {
    let mut buffer = [0; 4096];
    let mut pending = Vec::with_capacity(4);
    loop {
        match pipe.read(&mut buffer).await {
            Ok(0) => {
                let _ = emit_utf8(&mut pending, stderr, &events, true).await;
                break;
            }
            Err(_) => {
                let _ = emit_utf8(&mut pending, stderr, &events, true).await;
                break;
            }
            Ok(count) => {
                pending.extend_from_slice(&buffer[..count]);
                if !emit_utf8(&mut pending, stderr, &events, false).await { break; }
            }
        }
    }
}
async fn emit_utf8(pending: &mut Vec<u8>, stderr: bool, events: &mpsc::Sender<RunEvent>, final_chunk: bool) -> bool {
    loop {
        match std::str::from_utf8(pending) {
            Ok(text) => {
                if text.is_empty() { return true; }
                let text = text.to_owned();
                pending.clear();
                return events.send(RunEvent::Output { stderr, text }).await.is_ok();
            }
            Err(error) => {
                let valid = error.valid_up_to();
                if valid > 0 {
                    let text = std::str::from_utf8(&pending[..valid]).unwrap_or_default().to_owned();
                    pending.drain(..valid);
                    if events.send(RunEvent::Output { stderr, text }).await.is_err() { return false; }
                    continue;
                }
                if let Some(invalid) = error.error_len() {
                    pending.drain(..invalid);
                    if events.send(RunEvent::Output { stderr, text: "�".into() }).await.is_err() { return false; }
                    continue;
                }
                if final_chunk && !pending.is_empty() {
                    let text = String::from_utf8_lossy(pending).into_owned();
                    pending.clear();
                    return events.send(RunEvent::Output { stderr, text }).await.is_ok();
                }
                return true;
            }
        }
    }
}
fn kill_group(pid: Option<u32>) {
    #[cfg(unix)] if let Some(pid) = pid.filter(|pid| *pid > 1) {
        // SAFETY: the child was placed in its own process group at spawn.
        unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL); }
    }
    #[cfg(not(unix))] let _ = pid;
}

pub struct ConfigurationEditor {
    pub config: RunConfiguration,
    pub program_args: String,
    pub vm_args: String,
    pub environment: String,
    pub cwd: String,
    cwd_edited: bool,
    pub module: String,
}
impl ConfigurationEditor {
    fn new(config: RunConfiguration) -> Self {
        Self { program_args: serde_json::to_string(&config.program_args).unwrap_or_default(),
            vm_args: serde_json::to_string(&config.vm_args).unwrap_or_default(),
            environment: config.environment.iter().map(|(key, value)| format!("{key}={value}")).collect::<Vec<_>>().join("\n"),
            cwd: config.working_directory.to_string_lossy().into_owned(), cwd_edited: false,
            module: config.module.clone().unwrap_or_default(), config }
    }
    pub fn finish(&self) -> Result<RunConfiguration, RunError> {
        let mut config = self.config.clone();
        config.program_args = serde_json::from_str(&self.program_args).map_err(|error| RunError(format!("Program arguments must be a JSON array of strings: {error}")))?;
        config.vm_args = serde_json::from_str(&self.vm_args).map_err(|error| RunError(format!("VM arguments must be a JSON array of strings: {error}")))?;
        if self.cwd_edited { config.working_directory = PathBuf::from(&self.cwd); }
        config.module = (!self.module.trim().is_empty()).then(|| self.module.trim().to_owned());
        config.environment.clear();
        for line in self.environment.lines().filter(|line| !line.is_empty()) {
            let (key, value) = line.split_once('=').ok_or_else(|| RunError("Environment entries use NAME=value, one per line".into()))?;
            if config.environment.insert(key.to_owned(), value.to_owned()).is_some() { return Err(RunError(format!("Duplicate environment key: {key}"))); }
        }
        config.validate()?;
        Ok(config)
    }
}
#[derive(Default)]
pub struct RunTool {
    pub configurations: Vec<RunConfiguration>,
    pub selected: Option<String>,
    pub editor: Option<ConfigurationEditor>,
    pub configuration_error: Option<String>,
    pub loading: bool,
    pub saving: bool,
    writable: bool,
    pub summary: String,
    pub output: String,
    pub working_directory: Option<PathBuf>,
    pub jdk: String,
    active: Option<RunHandle>,
    retiring: Vec<RunHandle>,
}
impl RunTool {
    pub fn begin_project_load(&mut self) {
        self.cancel();
        if let Some(handle) = self.active.take() { self.retiring.push(handle); }
        self.output.clear(); self.summary.clear(); self.working_directory = None; self.jdk.clear(); self.editor = None;
        self.configurations.clear();
        self.configuration_error = None;
        self.writable = false;
        self.loading = true;
        self.saving = false;
        self.selected = self.configurations.first().map(|config| config.id.clone());
    }
    pub fn finish_project_load(&mut self, result: Result<Vec<RunConfiguration>, String>) {
        self.loading = false;
        match result {
            Ok(configurations) => { self.configurations = configurations; self.configuration_error = None; self.writable = true; }
            Err(error) => { self.configurations.clear(); self.configuration_error = Some(format!("{error}. Fix the file and reopen the project before editing configurations.")); self.writable = false; }
        }
        self.selected = self.configurations.first().map(|config| config.id.clone());
    }
    pub fn selected(&self) -> Option<&RunConfiguration> { self.configurations.iter().find(|config| Some(&config.id) == self.selected.as_ref()) }
    pub fn configurations_after_save(&self, config: RunConfiguration) -> Result<Vec<RunConfiguration>, RunError> {
        if !self.writable { return Err(RunError("Run configurations are protected after a load failure".into())); }
        let mut next = self.configurations.clone();
        if let Some(existing) = next.iter_mut().find(|existing| existing.id == config.id) { *existing = config.clone(); }
        else { next.push(config.clone()); }
        Ok(next)
    }
    pub fn configurations_after_delete(&self) -> Result<Vec<RunConfiguration>, RunError> {
        if !self.writable { return Err(RunError("Run configurations are protected after a load failure".into())); }
        Ok(self.configurations.iter().filter(|config| Some(&config.id) != self.selected.as_ref()).cloned().collect())
    }
    pub fn finish_save(&mut self, result: Result<Vec<RunConfiguration>, String>, selected: Option<String>) {
        self.saving = false;
        match result {
            Ok(configurations) => {
                self.configurations = configurations;
                self.selected = selected;
                self.editor = None;
                self.configuration_error = None;
            }
            Err(error) => self.configuration_error = Some(error),
        }
    }
    pub fn start(&mut self, service: RunService, config: &RunConfiguration, project: &ProjectModel, jdk: &JdkInstallation) -> Result<(), RunError> {
        if self.has_workers() { return Err(RunError("Wait for the previous run to finish stopping".into())); }
        let handle = service.start(config, project, jdk)?;
        self.active = Some(handle); self.output.clear(); self.summary = "Preparing Java run…".into();
        self.working_directory = Some(config.working_directory_at(&project.root));
        self.jdk = format!("{} — {}", jdk.label(), jdk.home.display());
        Ok(())
    }
    pub fn cancel(&self) { if let Some(handle) = &self.active { handle.cancel(); } for handle in &self.retiring { handle.cancel(); } }
    pub fn has_workers(&self) -> bool { self.active.is_some() || !self.retiring.is_empty() }
    pub fn poll(&mut self) {
        self.retiring.retain_mut(|handle| { handle.poll(); handle.reap().is_none() });
        let Some(handle) = &mut self.active else { return; };
        let mut events = handle.poll();
        let done = handle.reap();
        if done.is_some() { events.extend(handle.poll()); }
        for event in events {
            match event {
                RunEvent::Output { stderr, text } => { if stderr { self.output.push_str("[stderr] "); } self.output.push_str(&text); }
                RunEvent::State(text) => self.summary = text,
                RunEvent::Exit { code, cancelled } => self.summary = format!("{} — exit {}", if cancelled { "Stopped" } else { "Finished" }, code.map(|code| code.to_string()).unwrap_or_else(|| "signal".into())),
            }
        }
        if self.output.len() > 1_048_576 { let mut boundary = self.output.len() - 1_048_576; while !self.output.is_char_boundary(boundary) { boundary += 1; } self.output.drain(..boundary); }
        if let Some(result) = done { if let Err(error) = result { self.summary = error.to_string(); } self.active = None; }
    }
}

pub fn configuration_selector(ui: &mut eframe::egui::Ui, tool: &mut RunTool) {
    eframe::egui::ComboBox::from_id_salt("run_configuration_selector")
        .selected_text(tool.selected().map(|config| config.name.as_str()).unwrap_or("Run configuration…"))
        .show_ui(ui, |ui| {
            for config in &tool.configurations { ui.selectable_value(&mut tool.selected, Some(config.id.clone()), &config.name); }
        });
}
pub fn render(ui: &mut eframe::egui::Ui, state: &mut crate::app::AppState) {
    use crate::app::events::{AppEvent, RunAction};
    use eframe::egui;
    let Some(root) = state.project.as_ref().map(|project| project.root.clone()) else { ui.label("Open a Java project to configure a run."); return; };
    let project_model = state.project.as_ref().and_then(|project| project.model.as_ref());
    if state.run.loading { ui.horizontal(|ui| { ui.spinner(); ui.label("Loading run configurations…"); }); }
    if state.run.saving { ui.horizontal(|ui| { ui.spinner(); ui.label("Saving run configurations…"); }); }
    let mut intents = Vec::new();
    ui.horizontal_wrapped(|ui| {
        let can_edit = state.run.writable && !state.run.loading && !state.run.saving;
        ui.add_enabled_ui(!state.run.loading && !state.run.saving, |ui| configuration_selector(ui, &mut state.run));
        if ui.add_enabled(can_edit, egui::Button::new("New")).clicked() { state.run.editor = Some(ConfigurationEditor::new(RunConfiguration::new(&root))); }
        let selected = state.run.selected().cloned();
        if ui.add_enabled(can_edit && selected.is_some(), egui::Button::new("Edit")).clicked() {
            state.run.editor = selected.clone().map(ConfigurationEditor::new);
        }
        if ui.add_enabled(can_edit && selected.is_some(), egui::Button::new("Duplicate")).clicked() {
            if let Some(mut config) = selected.clone() { config.id = fresh_id(); config.name.push_str(" copy"); state.run.editor = Some(ConfigurationEditor::new(config)); }
        }
        if ui.add_enabled(can_edit && selected.is_some(), egui::Button::new("Delete")).clicked() { intents.push(AppEvent::Run(RunAction::Delete)); }
        if ui.add_enabled(selected.is_some() && !state.run.has_workers(), egui::Button::new("Run")).clicked() { intents.push(AppEvent::Run(RunAction::Start)); }
        if ui.add_enabled(state.run.has_workers(), egui::Button::new("Stop")).clicked() { intents.push(AppEvent::Run(RunAction::Stop)); }
    });
    if let Some(config) = state.run.selected() {
        match state.settings.run_jdk(&root, config) {
            Ok(jdk) => { ui.small(format!("Configuration JDK: {} — {}", jdk.label(), jdk.home.display())); }
            Err(error) => { ui.colored_label(ui.visuals().error_fg_color, error); }
        }
    }
    if let Some(error) = &state.run.configuration_error { ui.colored_label(ui.visuals().error_fg_color, error); }
    let can_save_configuration = !state.run.loading && !state.run.saving;
    let mut close_editor = false;
    if let Some(editor) = &mut state.run.editor {
        egui::CollapsingHeader::new("Edit run configuration").default_open(true).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("run_configuration_form").max_height(280.0).show(ui, |ui| {
                egui::Grid::new("run_fields").num_columns(2).show(ui, |ui| {
                    ui.label("Name"); ui.text_edit_singleline(&mut editor.config.name); ui.end_row();
                    ui.label("Main class"); ui.text_edit_singleline(&mut editor.config.main_class); ui.end_row();
                    ui.label("Build module");
                    let selected_module = editor.config.project_module.as_ref().map(|path| {
                        if path.as_os_str().is_empty() { "Root project".to_owned() }
                        else { project_model.and_then(|model| model.modules.iter().find(|module| module.root.strip_prefix(&model.root).ok() == Some(path.as_path()))).map(|module| module.name.clone()).unwrap_or_else(|| format!("Missing module ({})", path.display())) }
                    }).unwrap_or_else(|| "Automatic from main class".into());
                    egui::ComboBox::from_id_salt("run_project_module").selected_text(selected_module).show_ui(ui, |ui| {
                        ui.selectable_value(&mut editor.config.project_module, None, "Automatic from main class");
                        ui.selectable_value(&mut editor.config.project_module, Some(PathBuf::new()), "Root project");
                        if let Some(model) = project_model {
                            for module in &model.modules {
                                if let Ok(relative) = module.root.strip_prefix(&model.root) {
                                    ui.selectable_value(&mut editor.config.project_module, Some(relative.to_path_buf()), format!("{} ({})", module.name, relative.display()));
                                }
                            }
                        }
                    });
                    ui.end_row();
                    ui.label("Java module (JPMS, optional)"); ui.text_edit_singleline(&mut editor.module); ui.end_row();
                    ui.label("Program arguments (JSON)"); ui.text_edit_singleline(&mut editor.program_args); ui.end_row();
                    ui.label("VM arguments (JSON)"); ui.text_edit_singleline(&mut editor.vm_args); ui.end_row();
                    ui.label("Working directory");
                    ui.horizontal(|ui| {
                        if ui.text_edit_singleline(&mut editor.cwd).changed() { editor.cwd_edited = true; }
                        if ui.button("Browse…").clicked() {
                            let dialog = rfd::FileDialog::new().set_title("Select working directory");
                            if let Some(path) = dialog.pick_folder() {
                                editor.config.working_directory = path.clone();
                                editor.cwd = path.to_string_lossy().into_owned();
                                editor.cwd_edited = false;
                            }
                        }
                    });
                    ui.end_row();
                    ui.label("JDK");
                    let label = editor.config.jdk_id.as_ref().map(|id| state.settings.jdks.iter().find(|jdk| &jdk.id == id)
                        .map(|jdk| jdk.label()).unwrap_or_else(|| format!("Unavailable: {id}"))).unwrap_or_else(|| "Project default".into());
                    egui::ComboBox::from_id_salt("run_jdk_override").selected_text(label).show_ui(ui, |ui| {
                        ui.selectable_value(&mut editor.config.jdk_id, None, "Project default");
                        for jdk in &state.settings.jdks {
                            ui.selectable_value(&mut editor.config.jdk_id, Some(jdk.id.clone()), format!("{} — {}", jdk.label(), jdk.home.display()));
                        }
                    });
                    ui.end_row();
                    ui.label("Environment (NAME=value)"); ui.text_edit_multiline(&mut editor.environment); ui.end_row();
                });
                ui.small("Arguments: [\"one argument\", \"two\"]. Relative working directories use the Java build root. Build before running.");
                ui.horizontal(|ui| {
                    if ui.add_enabled(can_save_configuration, egui::Button::new("Save configuration")).clicked() {
                        match editor.finish() {
                            Ok(config) => intents.push(AppEvent::Run(RunAction::Save(config))),
                            Err(error) => state.run.configuration_error = Some(error.to_string()),
                        }
                    }
                    if ui.button("Cancel edit").clicked() { close_editor = true; }
                });
            });
        });
    }
    if close_editor { state.run.editor = None; }
    ui.separator();
    ui.label(&state.run.summary);
    if let Some(cwd) = &state.run.working_directory { ui.small(format!("Working directory: {}", cwd.display())); }
    if !state.run.jdk.is_empty() { ui.small(format!("Launch JDK: {}", state.run.jdk)); }
    egui::ScrollArea::both().id_salt("run_output").stick_to_bottom(true).show(ui, |ui| { ui.monospace(&state.run.output); });
    for intent in intents { state.enqueue(intent); }
}
