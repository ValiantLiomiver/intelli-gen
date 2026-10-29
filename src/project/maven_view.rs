//! Maven's effective model is resolved on workers so the UI only polls typed results.
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::java::jdk::JdkInstallation;

use super::maven_cache::{MavenViewCache, fingerprint_poms};
use super::{ProjectKind, ProjectModel};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MavenView {
    pub profiles: Vec<MavenProfile>,
    pub root_module: MavenModuleView,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MavenProfile {
    pub id: String,
    pub active_by_default: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MavenModuleView {
    pub name: String,
    pub relative_path: Option<PathBuf>,
    pub lifecycle: Vec<MavenTask>,
    pub plugins: Vec<MavenPluginView>,
    pub dependencies: Vec<String>,
    pub repositories: Vec<String>,
    pub modules: Vec<MavenModuleView>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MavenPluginView {
    pub name: String,
    pub tasks: Vec<MavenTask>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MavenTask {
    pub label: String,
    pub goal: String,
}

#[derive(Clone, Debug)]
pub struct MavenViewRequest {
    pub project: ProjectModel,
    pub executable: Option<PathBuf>,
    pub jdk: JdkInstallation,
    pub profiles: Vec<String>,
    /// When false, profiles marked activeByDefault are selected for the first load.
    pub profiles_user_modified: bool,
    pub generation: u64,
}

#[derive(Debug)]
pub struct MavenViewEvent {
    pub root: PathBuf,
    pub generation: u64,
    pub result: Result<MavenView, String>,
    pub cache_error: Option<String>,
}

pub struct MavenViewLoader {
    sender: Sender<MavenViewEvent>,
    receiver: Receiver<MavenViewEvent>,
    latest: HashMap<PathBuf, u64>,
    workers: Vec<MavenViewWorker>,
    generations: HashMap<PathBuf, Arc<AtomicU64>>,
}

struct MavenViewWorker {
    root: PathBuf,
    cancel: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl Default for MavenViewLoader {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            sender,
            receiver,
            latest: HashMap::new(),
            workers: Vec::new(),
            generations: HashMap::new(),
        }
    }
}

impl MavenViewLoader {
    pub fn start(&mut self, request: MavenViewRequest) -> Result<(), String> {
        if request.project.kind != ProjectKind::Maven {
            return Err("The Maven sidebar requires a Maven project".into());
        }
        let root = request.project.root.clone();
        let generation = request.generation;
        if self
            .latest
            .get(&root)
            .is_some_and(|generation| *generation >= request.generation)
        {
            return Err("Maven view generation must increase for each request".into());
        }
        let sender = self.sender.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let generation_token = self
            .generations
            .entry(root.clone())
            .or_insert_with(|| Arc::new(AtomicU64::new(0)))
            .clone();
        generation_token.store(generation, Ordering::Release);
        let worker_generation = Arc::clone(&generation_token);
        let worker = thread::Builder::new()
            .name("maven-view".into())
            .spawn(move || {
                let cache_project = request.project.clone();
                let fingerprint_before = fingerprint_poms(&cache_project);
                let event = MavenViewEvent {
                    root: request.project.root.clone(),
                    generation: request.generation,
                    result: load(request, Arc::clone(&worker_cancel)),
                    cache_error: None,
                };
                let mut event = event;
                if let Ok(view) = &event.result {
                    event.cache_error = match (fingerprint_before, fingerprint_poms(&cache_project))
                    {
                        (Ok(before), Ok(after))
                            if before == after
                                && worker_generation.load(Ordering::Acquire) == generation
                                && !worker_cancel.load(Ordering::Acquire) =>
                        {
                            MavenViewCache::for_user()
                                .and_then(|cache| cache.save(&cache_project.root, view, &after))
                                .err()
                        }
                        (Ok(_), Ok(_))
                            if worker_generation.load(Ordering::Acquire) != generation
                                || worker_cancel.load(Ordering::Acquire) =>
                        {
                            None
                        }
                        (Ok(_), Ok(_)) => Some(
                            "POM files changed while Maven was resolving; result was not cached"
                                .into(),
                        ),
                        (Err(error), _) | (_, Err(error)) => Some(error),
                    };
                }
                let _ = sender.send(event);
            })
            .map_err(|error| format!("Cannot start Maven view worker: {error}"))?;
        for previous in &self.workers {
            if previous.root == root {
                previous.cancel.store(true, Ordering::Release);
            }
        }
        self.latest.insert(root.clone(), generation);
        self.workers.push(MavenViewWorker {
            root,
            cancel,
            handle: worker,
        });
        Ok(())
    }

    /// Signal every worker. `poll` joins workers after their Maven children exit.
    pub fn cancel(&mut self) {
        self.latest.clear();
        for generation in self.generations.values() {
            generation.fetch_add(1, Ordering::AcqRel);
        }
        for worker in &self.workers {
            worker.cancel.store(true, Ordering::Release);
        }
    }

    pub fn poll(&mut self) -> Vec<MavenViewEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.receiver.try_recv() {
            if self.latest.get(&event.root) == Some(&event.generation) {
                events.push(event);
            }
        }
        let mut index = 0;
        while index < self.workers.len() {
            if self.workers[index].handle.is_finished() {
                let _ = self.workers.swap_remove(index).handle.join();
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

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);
impl Scratch {
    fn create() -> Result<Self, String> {
        for _ in 0..16 {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "intelli-gen-maven-{}-{timestamp}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(format!(
                        "Cannot create Maven scratch directory {}: {error}",
                        path.display()
                    ));
                }
            }
        }
        Err("Cannot allocate a unique Maven scratch directory".into())
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Default, Deserialize)]
struct Pom {
    name: Option<String>,
    #[serde(rename = "artifactId")]
    artifact_id: Option<String>,
    modules: Option<Modules>,
    profiles: Option<Profiles>,
    build: Option<Build>,
    dependencies: Option<Dependencies>,
    repositories: Option<Repositories>,
}

#[derive(Default, Deserialize)]
struct Modules {
    #[serde(default, rename = "module")]
    entries: Vec<String>,
}
#[derive(Default, Deserialize)]
struct Profiles {
    #[serde(default, rename = "profile")]
    entries: Vec<Profile>,
}
#[derive(Default, Deserialize)]
struct Profile {
    id: Option<String>,
    activation: Option<Activation>,
}
#[derive(Default, Deserialize)]
struct Activation {
    #[serde(rename = "activeByDefault")]
    active_by_default: Option<bool>,
}
#[derive(Default, Deserialize)]
struct Build {
    plugins: Option<Plugins>,
}
#[derive(Default, Deserialize)]
struct Plugins {
    #[serde(default, rename = "plugin")]
    entries: Vec<Plugin>,
}
#[derive(Default, Deserialize)]
struct Plugin {
    #[serde(rename = "groupId")]
    group_id: Option<String>,
    #[serde(rename = "artifactId")]
    artifact_id: Option<String>,
    version: Option<String>,
    executions: Option<Executions>,
}
#[derive(Default, Deserialize)]
struct Executions {
    #[serde(default, rename = "execution")]
    entries: Vec<Execution>,
}
#[derive(Default, Deserialize)]
struct Execution {
    goals: Option<Goals>,
}
#[derive(Default, Deserialize)]
struct Goals {
    #[serde(default, rename = "goal")]
    entries: Vec<String>,
}
#[derive(Default, Deserialize)]
struct Dependencies {
    #[serde(default, rename = "dependency")]
    entries: Vec<Dependency>,
}
#[derive(Default, Deserialize)]
struct Dependency {
    #[serde(rename = "groupId")]
    group_id: Option<String>,
    #[serde(rename = "artifactId")]
    artifact_id: Option<String>,
    version: Option<String>,
    scope: Option<String>,
}
#[derive(Default, Deserialize)]
struct Repositories {
    #[serde(default, rename = "repository")]
    entries: Vec<Repository>,
}
#[derive(Default, Deserialize)]
struct Repository {
    id: Option<String>,
    url: Option<String>,
}

struct Context {
    root: PathBuf,
    program: PathBuf,
    environment: Vec<(OsString, OsString)>,
    cancellation: Arc<AtomicBool>,
    profiles: Vec<String>,
    scratch: Scratch,
    next_output: usize,
    seen_modules: HashSet<PathBuf>,
    profile_ids: HashMap<String, bool>,
    plugin_goals: HashMap<(PathBuf, String), (String, Vec<String>)>,
}

fn load(request: MavenViewRequest, cancellation: Arc<AtomicBool>) -> Result<MavenView, String> {
    check_cancel(&cancellation)?;
    let root = request.project.root.clone();
    let profiles_user_modified = request.profiles_user_modified;
    for profile in &request.profiles {
        if !valid_profile_id(profile) {
            return Err(format!(
                "Cannot activate Maven profile {profile:?}: invalid profile ID for -P"
            ));
        }
    }
    let program = request
        .project
        .wrapper
        .or(request.executable)
        .ok_or("No Maven wrapper. Configure the Maven executable in Settings.")?;
    let program = if program.is_absolute() {
        program
    } else if program.components().count() > 1 {
        root.join(program)
    } else {
        program
    };
    let environment = request
        .jdk
        .child_environment()
        .map_err(|error| error.to_string())?;
    let mut context = Context {
        root: root.clone(),
        program,
        environment,
        cancellation,
        profiles: request.profiles,
        scratch: Scratch::create()?,
        next_output: 0,
        seen_modules: HashSet::new(),
        profile_ids: HashMap::new(),
        plugin_goals: HashMap::new(),
    };
    context.read_profiles(&root)?;
    // Include profiles in statically discovered POMs even if a different profile
    // currently excludes those modules from the effective reactor.
    for module in &request.project.modules {
        context.check_cancel()?;
        if module.root.join("pom.xml").is_file() {
            context.read_profiles(&module.root)?;
        }
    }
    if !profiles_user_modified {
        context.profiles.extend(
            context
                .profile_ids
                .iter()
                .filter(|(_, active_by_default)| **active_by_default)
                .map(|(id, _)| id.clone()),
        );
    }
    if context.profile_ids.contains_key("local")
        && !context.profiles.iter().any(|profile| profile == "local")
    {
        context.profiles.push("local".into());
    }
    let root_module = context.module(&root, true)?;
    let mut profiles: Vec<_> = context
        .profile_ids
        .into_iter()
        .map(|(id, active_by_default)| MavenProfile {
            id,
            active_by_default,
        })
        .collect();
    profiles.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(MavenView {
        profiles,
        root_module,
    })
}

impl Context {
    fn check_cancel(&self) -> Result<(), String> {
        check_cancel(&self.cancellation)
    }

    fn read_profiles(&mut self, directory: &Path) -> Result<(), String> {
        self.check_cancel()?;
        for profile in profile_definitions(directory)? {
            if !valid_profile_id(&profile.id) {
                return Err(format!(
                    "Cannot use Maven profile {:?} from {}: invalid profile ID for -P",
                    profile.id,
                    directory.join("pom.xml").display()
                ));
            }
            *self.profile_ids.entry(profile.id).or_default() |= profile.active_by_default;
        }
        Ok(())
    }

    fn module(&mut self, directory: &Path, root_module: bool) -> Result<MavenModuleView, String> {
        self.check_cancel()?;
        let identity = directory.canonicalize().map_err(|error| {
            format!(
                "Cannot resolve Maven module {}: {error}",
                directory.display()
            )
        })?;
        if !self.seen_modules.insert(identity.clone()) {
            return Err(format!(
                "Maven module cycle or duplicate at {}",
                directory.display()
            ));
        }
        if !root_module {
            self.read_profiles(directory)?;
        }
        let output = self.output_path("effective-pom.xml");
        let mut args = self.base_args();
        args.push(OsString::from("-N"));
        args.push(OsString::from("-f"));
        args.push(directory.join("pom.xml").into_os_string());
        args.push(OsString::from("help:effective-pom"));
        args.push(property_path("-Doutput=", &output));
        self.run(&args, "effective POM")?;
        let text = fs::read_to_string(&output)
            .map_err(|error| format!("Cannot read effective POM {}: {error}", output.display()))?;
        let pom: Pom = quick_xml::de::from_str(&text).map_err(|error| {
            format!(
                "Cannot parse effective POM for {}: {error}",
                directory.display()
            )
        })?;
        let relative_path = if root_module {
            None
        } else {
            let reactor = self.root.canonicalize().map_err(|error| {
                format!(
                    "Cannot resolve Maven reactor {}: {error}",
                    self.root.display()
                )
            })?;
            if !identity.starts_with(&reactor) || identity == reactor {
                return Err(format!(
                    "Maven module {} is outside the reactor root",
                    directory.display()
                ));
            }
            let relative = directory.strip_prefix(&self.root).map_err(|_| {
                format!(
                    "Maven module {} is outside the reactor root",
                    directory.display()
                )
            })?;
            let mut normalized = PathBuf::new();
            for component in relative.components() {
                match component {
                    Component::CurDir => {}
                    Component::Normal(part) => normalized.push(part),
                    Component::ParentDir if normalized.pop() => {}
                    _ => {
                        return Err(format!(
                            "Maven module {} is outside the reactor root",
                            directory.display()
                        ));
                    }
                }
            }
            if normalized.as_os_str().is_empty() {
                return Err(format!(
                    "Maven module {} resolves to the reactor root",
                    directory.display()
                ));
            }
            Some(normalized)
        };
        let name = if root_module {
            "main_module".into()
        } else {
            pom.name
                .clone()
                .or(pom.artifact_id.clone())
                .unwrap_or_else(|| {
                    directory
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                })
        };
        let mut plugins = Vec::new();
        for plugin in pom
            .build
            .unwrap_or_default()
            .plugins
            .unwrap_or_default()
            .entries
        {
            self.check_cancel()?;
            let Some(artifact) = plugin.artifact_id else {
                continue;
            };
            let group = plugin
                .group_id
                .unwrap_or_else(|| "org.apache.maven.plugins".into());
            let coordinate = match plugin.version.as_deref() {
                Some(version) => format!("{group}:{artifact}:{version}"),
                None => format!("{group}:{artifact}"),
            };
            let (prefix, mut goals) = self
                .describe_goals(directory, &coordinate)
                .map_err(|error| format!("Cannot resolve goals for {coordinate}: {error}"))?;
            for execution in plugin.executions.unwrap_or_default().entries {
                goals.extend(execution.goals.unwrap_or_default().entries);
            }
            goals.sort();
            goals.dedup();
            let tasks = goals
                .into_iter()
                .map(|goal| MavenTask {
                    label: goal.clone(),
                    goal: if plugin.version.is_some() {
                        format!("{coordinate}:{goal}")
                    } else {
                        format!("{prefix}:{goal}")
                    },
                })
                .collect();
            plugins.push(MavenPluginView {
                name: artifact,
                tasks,
            });
        }
        let dependencies = pom
            .dependencies
            .unwrap_or_default()
            .entries
            .into_iter()
            .filter_map(|dependency| {
                let (Some(group), Some(artifact)) = (dependency.group_id, dependency.artifact_id)
                else {
                    return None;
                };
                let mut label = format!("{group}:{artifact}");
                if let Some(version) = dependency.version {
                    label.push(':');
                    label.push_str(&version);
                }
                if let Some(scope) = dependency.scope {
                    label.push_str(" [");
                    label.push_str(&scope);
                    label.push(']');
                }
                Some(label)
            })
            .collect();
        let repositories = pom
            .repositories
            .unwrap_or_default()
            .entries
            .into_iter()
            .filter_map(|repository| repository.id.or(repository.url))
            .collect();
        let mut modules = Vec::new();
        for entry in pom.modules.unwrap_or_default().entries {
            self.check_cancel()?;
            let entry = entry.trim();
            if entry.is_empty() || entry.contains("${") {
                return Err(format!(
                    "Invalid effective Maven module path in {}: {entry}",
                    directory.display()
                ));
            }
            modules.push(self.module(&directory.join(entry), false)?);
        }
        Ok(MavenModuleView {
            name,
            relative_path,
            lifecycle: lifecycle_tasks(),
            plugins,
            dependencies,
            repositories,
            modules,
        })
    }

    fn describe_goals(
        &mut self,
        directory: &Path,
        coordinate: &str,
    ) -> Result<(String, Vec<String>), String> {
        self.check_cancel()?;
        let key = (directory.to_path_buf(), coordinate.to_owned());
        if let Some(goals) = self.plugin_goals.get(&key) {
            return Ok(goals.clone());
        }
        let output = self.output_path("plugin.txt");
        let mut args = self.base_args();
        args.push(OsString::from("-N"));
        args.push(OsString::from("-f"));
        args.push(directory.join("pom.xml").into_os_string());
        args.push(OsString::from("help:describe"));
        args.push(OsString::from(format!("-Dplugin={coordinate}")));
        args.push(property_path("-Doutput=", &output));
        self.run(&args, "plugin goals")?;
        let description = fs::read_to_string(&output)
            .map_err(|error| format!("Cannot read Maven plugin description: {error}"))?;
        let prefix = description
            .lines()
            .find_map(|line| line.trim().strip_prefix("Goal Prefix:").map(str::trim))
            .filter(|prefix| !prefix.is_empty())
            .ok_or("Maven plugin description has no goal prefix")?;
        let marker = format!("{prefix}:");
        let goals: Vec<String> = description
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let rest = line.strip_prefix(&marker)?;
                let goal = rest.split_whitespace().next()?.trim_end_matches(':');
                (!goal.is_empty()
                    && goal
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'))
                .then(|| goal.to_owned())
            })
            .collect();
        if goals.is_empty() {
            return Err("Maven plugin description contains no recognizable goals".into());
        }
        let result = (prefix.to_owned(), goals);
        self.plugin_goals.insert(key, result.clone());
        Ok(result)
    }

    fn base_args(&self) -> Vec<OsString> {
        let mut args = vec![
            "--batch-mode".into(),
            "--no-transfer-progress".into(),
            "-Dstyle.color=never".into(),
        ];
        let mut selectors = self.profiles.clone();
        selectors.sort();
        selectors.dedup();
        let mut disabled: Vec<_> = self
            .profile_ids
            .keys()
            .filter(|id| id.as_str() != "local" && !self.profiles.contains(id))
            .map(|id| format!("!{id}"))
            .collect();
        disabled.sort();
        selectors.extend(disabled);
        if !selectors.is_empty() {
            args.push(OsString::from("-P"));
            args.push(OsString::from(selectors.join(",")));
        }
        args
    }

    fn output_path(&mut self, suffix: &str) -> PathBuf {
        self.next_output += 1;
        self.scratch
            .0
            .join(format!("{}-{suffix}", self.next_output))
    }

    fn run(&self, args: &[OsString], operation: &str) -> Result<(), String> {
        self.check_cancel()?;
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let stdout_path = self.scratch.0.join(format!("process-{sequence}.out"));
        let stderr_path = self.scratch.0.join(format!("process-{sequence}.err"));
        let stdout_file = fs::File::create(&stdout_path)
            .map_err(|error| format!("Cannot create Maven output file: {error}"))?;
        let stderr_file = fs::File::create(&stderr_path)
            .map_err(|error| format!("Cannot create Maven error file: {error}"))?;
        let mut command = Command::new(&self.program);
        command
            .args(args)
            .current_dir(&self.root)
            .envs(self.environment.iter().cloned())
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout_file))
            .stderr(Stdio::from(stderr_file));
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("Cannot launch Maven for {operation}: {error}"))?;
        let status = loop {
            if self.cancellation.load(Ordering::Acquire) {
                terminate_maven(&mut child);
                return Err("Maven view load cancelled".into());
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => thread::sleep(Duration::from_millis(25)),
                Err(error) => {
                    terminate_maven(&mut child);
                    return Err(format!("Cannot wait for Maven {operation}: {error}"));
                }
            }
        };
        // try_wait has already reaped the direct child. A wrapper may have left
        // descendants in its process group, so clear that group before the
        // metadata worker releases the app's close gate.
        #[cfg(unix)]
        cleanup_maven_group(child.id());
        self.check_cancel()?;
        if status.success() {
            return Ok(());
        }
        let stdout_bytes =
            fs::read(&stdout_path).map_err(|error| format!("Cannot read Maven output: {error}"))?;
        let stderr_bytes =
            fs::read(&stderr_path).map_err(|error| format!("Cannot read Maven errors: {error}"))?;
        self.check_cancel()?;
        let stdout = String::from_utf8_lossy(&stdout_bytes);
        let stderr = String::from_utf8_lossy(&stderr_bytes);
        let lines: Vec<_> = stdout
            .lines()
            .chain(stderr.lines())
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        let errors: Vec<_> = lines
            .iter()
            .copied()
            .filter(|line| line.contains("[ERROR]"))
            .take(12)
            .collect();
        let detail = if errors.is_empty() {
            let mut tail: Vec<_> = lines.iter().rev().take(8).copied().collect();
            tail.reverse();
            if tail.is_empty() {
                "No Maven diagnostics".to_owned()
            } else {
                tail.join("\n")
            }
        } else {
            errors.join("\n")
        };
        Err(format!("Maven {operation} failed ({}): {detail}", status))
    }
}

fn terminate_maven(child: &mut std::process::Child) {
    #[cfg(unix)]
    cleanup_maven_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn cleanup_maven_group(pid: u32) {
    if pid > 1 {
        // SAFETY: Maven was launched in a fresh process group with PID as PGID.
        // A missing group simply means its descendants have already exited.
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
    }
}

fn property_path(prefix: &str, path: &Path) -> OsString {
    let mut value = OsString::from(prefix);
    value.push(path.as_os_str());
    value
}

fn check_cancel(cancellation: &AtomicBool) -> Result<(), String> {
    if cancellation.load(Ordering::Acquire) {
        Err("Maven view load cancelled".into())
    } else {
        Ok(())
    }
}

fn valid_profile_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with(['!', '-', '?'])
        && !id
            .chars()
            .any(|ch| ch == ',' || ch.is_control() || ch.is_whitespace())
}

fn profile_definitions(directory: &Path) -> Result<Vec<MavenProfile>, String> {
    let path = directory.join("pom.xml");
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
    let pom: Pom = quick_xml::de::from_str(&text)
        .map_err(|error| format!("Cannot parse {}: {error}", path.display()))?;
    Ok(pom
        .profiles
        .unwrap_or_default()
        .entries
        .into_iter()
        .filter_map(|profile| {
            let id = profile.id.filter(|id| !id.trim().is_empty())?;
            let active_by_default = profile
                .activation
                .and_then(|activation| activation.active_by_default)
                .unwrap_or(false);
            Some(MavenProfile {
                id,
                active_by_default,
            })
        })
        .collect())
}

fn lifecycle_tasks() -> Vec<MavenTask> {
    [
        "pre-clean",
        "clean",
        "post-clean",
        "validate",
        "initialize",
        "generate-sources",
        "process-sources",
        "generate-resources",
        "process-resources",
        "compile",
        "process-classes",
        "generate-test-sources",
        "process-test-sources",
        "generate-test-resources",
        "process-test-resources",
        "test-compile",
        "process-test-classes",
        "test",
        "prepare-package",
        "package",
        "pre-integration-test",
        "integration-test",
        "post-integration-test",
        "verify",
        "install",
        "deploy",
        "pre-site",
        "site",
        "post-site",
        "site-deploy",
    ]
    .into_iter()
    .map(|phase| MavenTask {
        label: phase.into(),
        goal: phase.into(),
    })
    .collect()
}
