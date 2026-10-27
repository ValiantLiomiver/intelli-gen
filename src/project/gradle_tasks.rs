//! Gradle task reports are resolved on workers; stale project/module results are ignored.
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{ProjectKind, ProjectModel};
use crate::java::jdk::JdkInstallation;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GradleTask {
    pub path: String,
    pub description: String,
}

#[derive(Clone, Debug)]
pub struct GradleTaskEvent {
    pub root: PathBuf,
    pub project_path: Option<PathBuf>,
    pub generation: u64,
    pub result: Result<Vec<GradleTask>, String>,
}

pub struct GradleTaskLoader {
    sender: Sender<GradleTaskEvent>,
    receiver: Receiver<GradleTaskEvent>,
    latest: HashMap<PathBuf, u64>,
    workers: Vec<Worker>,
}

struct Worker {
    root: PathBuf,
    cancel: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl Default for GradleTaskLoader {
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

impl GradleTaskLoader {
    pub fn start(
        &mut self,
        project: ProjectModel,
        project_path: Option<PathBuf>,
        executable: Option<PathBuf>,
        jdk: JdkInstallation,
        generation: u64,
    ) -> Result<(), String> {
        if project.kind != ProjectKind::Gradle {
            return Err("The Gradle task browser requires a Gradle project".into());
        }
        if self
            .latest
            .get(&project.root)
            .is_some_and(|latest| *latest >= generation)
        {
            return Err("Gradle task generation must increase for each request".into());
        }
        let module_name = match &project_path {
            None => None,
            Some(path) => Some(
                project
                    .modules
                    .iter()
                    .find(|module| &module.root == path)
                    .ok_or("Selected directory is not a discovered Gradle project")?
                    .name
                    .clone(),
            ),
        };
        if module_name
            .as_deref()
            .is_some_and(|name| !valid_project_path(name))
        {
            return Err("Selected Gradle project has an unsupported task path".into());
        }
        let program = project
            .wrapper
            .as_deref()
            .or(executable.as_deref())
            .ok_or("No Gradle wrapper. Configure the Gradle executable in Settings.")?;
        let program = if program.is_absolute() {
            program.to_path_buf()
        } else if program.components().count() > 1 {
            project.root.join(program)
        } else {
            program.to_path_buf()
        };
        let environment = jdk.child_environment().map_err(|error| error.to_string())?;
        let root = project.root.clone();
        let worker_root = root.clone();
        let event_root = root.clone();
        let event_project = project_path.clone();
        let sender = self.sender.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker = thread::Builder::new()
            .name("gradle-task-report".into())
            .spawn(move || {
                let result = load_tasks(
                    &worker_root,
                    program,
                    module_name.as_deref(),
                    environment,
                    worker_cancel,
                );
                let _ = sender.send(GradleTaskEvent {
                    root: event_root,
                    project_path: event_project,
                    generation,
                    result,
                });
            })
            .map_err(|error| format!("Cannot start Gradle task worker: {error}"))?;
        for previous in &self.workers {
            if previous.root == root {
                previous.cancel.store(true, Ordering::Release);
            }
        }
        self.latest.insert(root.clone(), generation);
        self.workers.push(Worker {
            root,
            cancel,
            handle: worker,
        });
        Ok(())
    }

    pub fn cancel(&mut self) {
        self.latest.clear();
        for worker in &self.workers {
            worker.cancel.store(true, Ordering::Release);
        }
    }

    pub fn poll(&mut self) -> Vec<GradleTaskEvent> {
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

impl Drop for GradleTaskLoader {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct Scratch(PathBuf);
impl Scratch {
    fn create() -> Result<Self, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..16 {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "intelli-gen-gradle-{}-{stamp}-{id}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(format!(
                        "Cannot create Gradle task scratch directory: {error}"
                    ));
                }
            }
        }
        Err("Cannot allocate a unique Gradle task scratch directory".into())
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn load_tasks(
    root: &Path,
    program: PathBuf,
    module_name: Option<&str>,
    environment: Vec<(OsString, OsString)>,
    cancellation: Arc<AtomicBool>,
) -> Result<Vec<GradleTask>, String> {
    check_cancel(&cancellation)?;
    if let Some(module_name) = module_name {
        let module = module_name
            .strip_prefix(':')
            .ok_or("Invalid Gradle project path")?;
        let mut path = root.to_path_buf();
        for component in module.split(':') {
            if component.is_empty()
                || component == "."
                || component == ".."
                || component.contains(['/', '\\'])
            {
                return Err("Invalid Gradle project path".into());
            }
            path.push(component);
        }
        let reactor = root
            .canonicalize()
            .map_err(|error| format!("Cannot inspect Gradle project: {error}"))?;
        let resolved = path.canonicalize().map_err(|error| {
            format!(
                "Cannot inspect Gradle subproject {}: {error}",
                path.display()
            )
        })?;
        if !resolved.starts_with(&reactor) || resolved == reactor || !resolved.is_dir() {
            return Err("Gradle subproject is outside the opened build".into());
        }
    }
    let scratch = Scratch::create()?;
    let stdout_path = scratch.0.join("tasks.out");
    let stderr_path = scratch.0.join("tasks.err");
    let stdout = File::create(&stdout_path)
        .map_err(|error| format!("Cannot create Gradle output file: {error}"))?;
    let stderr = File::create(&stderr_path)
        .map_err(|error| format!("Cannot create Gradle error file: {error}"))?;
    let mut command = Command::new(program);
    command
        .args(["--console=plain", "--no-daemon"])
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .current_dir(root)
        .envs(environment);
    if let Some(module_name) = module_name {
        command.arg(format!("{module_name}:tasks"));
    } else {
        command.arg("tasks");
    }
    command.arg("--all");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("Cannot launch Gradle task report: {error}"))?;
    let pid = child.id();
    let status = loop {
        if cancellation.load(Ordering::Acquire) {
            terminate(&mut child);
            return Err("Gradle task report cancelled".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                terminate(&mut child);
                return Err(format!("Cannot wait for Gradle task report: {error}"));
            }
        }
    };
    #[cfg(unix)]
    cleanup_process_group(pid);
    check_cancel(&cancellation)?;
    let output = fs::read(&stdout_path)
        .map_err(|error| format!("Cannot read Gradle task report: {error}"))?;
    if !status.success() {
        let errors = fs::read(&stderr_path).unwrap_or_default();
        return Err(format!(
            "Gradle task report failed ({status}): {}",
            output_tail(&errors)
        ));
    }
    let prefix = module_name.unwrap_or(":");
    let tasks = parse_task_report(prefix, &String::from_utf8_lossy(&output));
    if tasks.is_empty() {
        return Err(format!(
            "Gradle returned no recognizable tasks for {prefix}"
        ));
    }
    Ok(tasks)
}

fn parse_task_report(project_path: &str, output: &str) -> Vec<GradleTask> {
    let mut in_report = false;
    let mut tasks = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.starts_with("Tasks runnable from ") {
            in_report = true;
            continue;
        }
        if !in_report {
            continue;
        }
        if line.is_empty() || line.chars().all(|character| character == '-') {
            continue;
        }
        if line.starts_with("> Task ")
            || line.starts_with("BUILD ")
            || line.starts_with("FAILURE: ")
        {
            in_report = false;
            continue;
        }
        let (name, description) = line.split_once(" - ").map_or((line, ""), |parts| parts);
        let name = name.trim();
        if !valid_task_name(name) {
            continue;
        }
        let prefix = if project_path == ":" {
            String::new()
        } else {
            project_path.trim_end_matches(':').to_owned()
        };
        let path = format!("{prefix}:{name}");
        tasks.push(GradleTask {
            path,
            description: description.trim().to_owned(),
        });
    }
    tasks.sort_by(|left, right| left.path.cmp(&right.path));
    tasks.dedup_by(|left, right| left.path == right.path);
    tasks
}

fn valid_task_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn valid_project_path(path: &str) -> bool {
    path.starts_with(':') && path[1..].split(':').all(valid_task_name)
}

fn output_tail(bytes: &[u8]) -> String {
    let output = String::from_utf8_lossy(bytes);
    let mut lines: Vec<_> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.len() > 12 {
        lines.drain(..lines.len() - 12);
    }
    let text = lines.join("\n");
    if text.len() > 4096 {
        let mut start = text.len() - 4096;
        while !text.is_char_boundary(start) {
            start += 1;
        }
        text[start..].to_owned()
    } else {
        text
    }
}

fn check_cancel(cancellation: &AtomicBool) -> Result<(), String> {
    if cancellation.load(Ordering::Acquire) {
        Err("Gradle task report cancelled".into())
    } else {
        Ok(())
    }
}

fn terminate(child: &mut std::process::Child) {
    #[cfg(unix)]
    cleanup_process_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn cleanup_process_group(pid: u32) {
    if pid > 1 {
        // SAFETY: Gradle was launched in a fresh process group with PID as PGID.
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
    }
}
