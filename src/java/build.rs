//! Build processes run on a private Tokio worker; the UI only drains events.
use std::collections::VecDeque;
use std::ffi::OsString;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::{mpsc, watch};

use crate::project::{ProjectKind, ProjectModel};
use super::diagnostics::{self, BuildDiagnostic};
use super::jdk::JdkInstallation;

#[derive(Clone, Copy, Debug)]
pub enum BuildTask { Build, Test, Clean }

impl fmt::Display for BuildTask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self { Self::Build => "Build", Self::Test => "Test", Self::Clean => "Clean" })
    }
}

#[derive(Clone, Debug)]
pub struct BuildRequest {
    pub project: ProjectModel,
    pub task: BuildTask,
    pub jdk: JdkInstallation,
}

#[derive(Clone, Debug)]
pub struct MavenTaskRequest {
    pub project: ProjectModel,
    pub goal: String,
    pub module: Option<PathBuf>,
    pub profiles: Vec<String>,
    pub disabled_profiles: Vec<String>,
    pub jdk: JdkInstallation,
}

#[derive(Clone, Debug)]
pub struct GradleTaskRequest {
    pub project: ProjectModel,
    pub task_path: String,
    pub jdk: JdkInstallation,
}

#[derive(Clone, Debug)]
pub enum BuildEvent {
    Output { stderr: bool, text: String },
    Diagnostic(BuildDiagnostic),
    Tests(diagnostics::TestSummary),
    TestFailures(Vec<TestFailure>),
    Finished {
        exit_code: Option<i32>, elapsed: Duration, cancelled: bool,
        jdk: String, error: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestFailure {
    pub class_name: String,
    pub test_name: String,
    pub message: String,
    pub source_path: Option<PathBuf>,
    pub source_line: Option<usize>,
}

#[derive(serde::Deserialize)]
struct XmlTestReport {
    #[serde(rename = "@name")]
    name: Option<String>,
    #[serde(rename = "testsuite", default)]
    suites: Vec<XmlTestReport>,
    #[serde(rename = "testcase", default)]
    cases: Vec<XmlTestCase>,
}

#[derive(serde::Deserialize)]
struct XmlTestCase {
    #[serde(rename = "@classname")]
    class_name: Option<String>,
    #[serde(rename = "@name")]
    name: Option<String>,
    #[serde(rename = "failure", default)]
    failures: Vec<XmlTestMessage>,
    #[serde(rename = "error", default)]
    errors: Vec<XmlTestMessage>,
}

#[derive(serde::Deserialize)]
struct XmlTestMessage {
    #[serde(rename = "@message")]
    message: Option<String>,
    #[serde(rename = "$text")]
    text: Option<String>,
}

#[derive(Debug)]
pub struct BuildError(pub String);
impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.0) }
}
impl std::error::Error for BuildError {}

pub struct BuildHandle {
    events: mpsc::Receiver<BuildEvent>,
    cancel: watch::Sender<bool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl BuildHandle {
    pub fn cancel(&self) { let _ = self.cancel.send(true); }
    pub fn try_event(&mut self) -> Result<BuildEvent, mpsc::error::TryRecvError> {
        self.events.try_recv()
    }
    /// Join only after the worker has returned, keeping process waits off the UI.
    fn poll_worker_completion(&mut self) -> Option<Result<(), String>> {
        if !self.worker.as_ref()?.is_finished() { return None; }
        self.worker.take().map(|worker| worker.join().map_err(|_| "Build worker panicked".to_owned()))
    }
}
impl Drop for BuildHandle {
    fn drop(&mut self) { self.cancel(); }
}

pub struct BuildService;
impl BuildService {
    pub fn start(request: BuildRequest) -> Result<BuildHandle, BuildError> {
        Self::start_with_executable(request, None)
    }

    /// A wrapper takes precedence. Without one, a configured executable is required.
    pub fn start_with_executable(request: BuildRequest, executable: Option<&Path>) -> Result<BuildHandle, BuildError> {
        let program = build_program(&request.project, executable)?;
        let args: &[&str] = match (request.project.kind, request.task) {
            (ProjectKind::Maven, BuildTask::Build) => &["--batch-mode", "--no-transfer-progress", "-Dstyle.color=never", "-DskipTests", "package"],
            (ProjectKind::Maven, BuildTask::Test) => &["--batch-mode", "--no-transfer-progress", "-Dstyle.color=never", "test"],
            (ProjectKind::Maven, BuildTask::Clean) => &["--batch-mode", "--no-transfer-progress", "-Dstyle.color=never", "clean"],
            (ProjectKind::Gradle, BuildTask::Build) => &["--console=plain", "--no-daemon", "build", "-x", "test"],
            (ProjectKind::Gradle, BuildTask::Test) => &["--console=plain", "--no-daemon", "test"],
            (ProjectKind::Gradle, BuildTask::Clean) => &["--console=plain", "--no-daemon", "clean"],
            (ProjectKind::Unknown, _) => return Err(BuildError("Open a Maven or Gradle build first".into())),
        };
        Self::launch(request.project, request.jdk, program, None,
            args.iter().map(|arg| OsString::from(*arg)).collect())
    }

    pub fn start_maven_task(request: MavenTaskRequest, executable: Option<&Path>) -> Result<BuildHandle, BuildError> {
        if request.project.kind != ProjectKind::Maven {
            return Err(BuildError("Open a Maven build first".into()));
        }
        if request.goal.is_empty() || request.goal.starts_with('-')
            || request.goal.chars().any(char::is_whitespace) {
            return Err(BuildError("Choose a valid Maven phase or plugin goal".into()));
        }
        if request.profiles.iter().chain(&request.disabled_profiles)
            .any(|profile| !valid_profile_id(profile))
            || request.disabled_profiles.iter().any(|profile| profile == "local"
                || request.profiles.contains(profile)) {
            return Err(BuildError("A Maven profile ID is invalid".into()));
        }
        let module = request.module.as_deref()
            .map(|module| reactor_module(&request.project.root, module)).transpose()?;
        let program = build_program(&request.project, executable)?;
        let mut args = vec![OsString::from("--batch-mode"), OsString::from("--no-transfer-progress"),
            OsString::from("-Dstyle.color=never")];
        let mut selectors = request.profiles.clone();
        selectors.sort();
        selectors.dedup();
        let mut disabled = request.disabled_profiles.iter()
            .map(|profile| format!("!{profile}"))
            .collect::<Vec<_>>();
        disabled.sort();
        disabled.dedup();
        selectors.extend(disabled);
        if !selectors.is_empty() {
            args.push(OsString::from("-P"));
            args.push(OsString::from(selectors.join(",")));
        }
        if let Some(module) = &module {
            args.push(OsString::from("-pl"));
            args.push(module.as_os_str().to_os_string());
            args.push(OsString::from("-am"));
        }
        args.push(OsString::from(request.goal));
        Self::launch(request.project, request.jdk, program, module, args)
    }

    pub fn start_gradle_task(request: GradleTaskRequest, executable: Option<&Path>) -> Result<BuildHandle, BuildError> {
        if request.project.kind != ProjectKind::Gradle {
            return Err(BuildError("Open a Gradle build first".into()));
        }
        if !valid_gradle_task_path(&request.task_path) {
            return Err(BuildError("Choose a valid discovered Gradle task path".into()));
        }
        let program = build_program(&request.project, executable)?;
        Self::launch(
            request.project,
            request.jdk,
            program,
            None,
            vec![OsString::from("--console=plain"), OsString::from("--no-daemon"), OsString::from(request.task_path)],
        )
    }

    fn launch(
        project: ProjectModel, jdk: JdkInstallation, program: PathBuf, module: Option<PathBuf>,
        args: Vec<OsString>,
    ) -> Result<BuildHandle, BuildError> {
        let (sender, events) = mpsc::channel(256);
        let (cancel, cancellation) = watch::channel(false);
        let worker = std::thread::Builder::new().name("java-build".into()).spawn(move || {
            match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime.block_on(run(program, args, project, jdk, module, sender, cancellation)),
                Err(error) => {
                    let label = format!("{} — {}", jdk.label(), jdk.home.display());
                    let _ = sender.blocking_send(BuildEvent::Finished { exit_code: None,
                        elapsed: Duration::ZERO, cancelled: false, jdk: label,
                        error: Some(format!("Cannot create build worker: {error}")) });
                }
            }
        }).map_err(|error| BuildError(format!("Cannot start build worker: {error}")))?;
        Ok(BuildHandle { events, cancel, worker: Some(worker) })
    }
}

fn valid_gradle_task_path(path: &str) -> bool {
    path.starts_with(':') && path[1..].split(':').all(|segment| {
        let mut bytes = segment.bytes();
        bytes.next().is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    })
}

fn build_program(project: &ProjectModel, executable: Option<&Path>) -> Result<PathBuf, BuildError> {
    let program = project.wrapper.as_deref().or(executable)
        .ok_or_else(|| BuildError(format!("No {} wrapper. Configure the executable in Settings.", project.kind)))?;
    Ok(if program.is_absolute() { program.to_path_buf() }
        else if program.components().count() > 1 { project.root.join(program) }
        else { program.to_path_buf() })
}

fn valid_profile_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with(['!', '-', '?'])
        && !id.chars().any(|ch| ch == ',' || ch.is_control() || ch.is_whitespace())
}

/// Return a reactor-relative path without allowing lexical traversal outside it.
fn reactor_module(root: &Path, module: &Path) -> Result<PathBuf, BuildError> {
    if !root.is_absolute() {
        return Err(BuildError("Maven reactor root must be absolute".into()));
    }
    let relative = if module.is_absolute() {
        module.strip_prefix(root).map_err(|_| BuildError("Maven module is outside the reactor".into()))?
    } else { module };
    let mut normalized = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir if normalized.pop() => {}
            _ => return Err(BuildError("Maven module is outside the reactor".into())),
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(BuildError("Choose a child Maven module or run the root task".into()));
    }
    Ok(normalized)
}

/// Filesystem checks happen on the build worker before the Maven process starts.
fn validate_module(root: &Path, module: &Path) -> Result<(), String> {
    let reactor = root.canonicalize().map_err(|error| format!("Cannot inspect Maven reactor: {error}"))?;
    let target = root.join(module).canonicalize()
        .map_err(|error| format!("Cannot inspect Maven module {}: {error}", module.display()))?;
    if !target.starts_with(&reactor) || target == reactor {
        return Err(format!("Maven module {} is outside the reactor", module.display()));
    }
    let pom = target.join("pom.xml");
    let metadata = std::fs::metadata(&pom)
        .map_err(|error| format!("Cannot inspect Maven module POM {}: {error}", pom.display()))?;
    if !metadata.is_file() {
        return Err(format!("Maven module POM {} is not a file", pom.display()));
    }
    Ok(())
}

async fn run(program: PathBuf, args: Vec<OsString>, project: ProjectModel, jdk_installation: JdkInstallation,
    module: Option<PathBuf>, events: mpsc::Sender<BuildEvent>, mut cancellation: watch::Receiver<bool>) {
    let start = Instant::now();
    let started_at = SystemTime::now();
    let jdk = format!("{} — {}", jdk_installation.label(), jdk_installation.home.display());
    let mut cancelled = false;
    let result: Result<ExitStatus, String> = async {
        if let Some(module) = &module { validate_module(&project.root, module)?; }
        let environment = jdk_installation.child_environment().map_err(|error| error.to_string())?;
        let mut command = tokio::process::Command::new(program);
        command.args(args).current_dir(&project.root).envs(environment)
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.as_std_mut().process_group(0);
        }
        let mut child = command.spawn().map_err(|error| format!("Cannot launch {}: {error}", project.kind))?;
        let pid = child.id();
        let stdout = child.stdout.take().ok_or("Missing build stdout")?;
        let stderr = child.stderr.take().ok_or("Missing build stderr")?;
        let output = tokio::spawn(read_output(stdout, false, project.root.clone(), events.clone()));
        let errors = tokio::spawn(read_output(stderr, true, project.root.clone(), events.clone()));
        let status = if *cancellation.borrow() {
            cancelled = true;
            terminate_group(pid);
            let _ = child.start_kill();
            child.wait().await
        } else {
            tokio::select! {
                status = child.wait() => status,
                _ = cancellation.changed() => {
                    cancelled = true;
                    terminate_group(pid);
                    let _ = child.start_kill();
                    child.wait().await
                }
            }
        };
        // Tool subprocesses must not keep pipe readers or single-use daemons alive.
        terminate_group(pid);
        let mut output = output;
        let mut errors = errors;
        if tokio::time::timeout(Duration::from_secs(2), async {
            let _ = (&mut output).await;
            let _ = (&mut errors).await;
        }).await.is_err() {
            output.abort();
            errors.abort();
            let _ = events.send(BuildEvent::Output { stderr: true,
                text: "Build output drain timed out; remaining descendant output was discarded.".into() }).await;
        }
        status.map_err(|error| format!("Cannot wait for build process: {error}"))
    }.await;
    let (exit_code, error) = match result {
        Ok(status) => (status.code(), None),
        Err(error) => (None, Some(error)),
    };
    if !cancelled {
        let failures = collect_test_failures(&project, started_at);
        if !failures.is_empty() {
            let _ = events.send(BuildEvent::TestFailures(failures)).await;
        }
    }
    let _ = events.send(BuildEvent::Finished { exit_code, elapsed: start.elapsed(), cancelled, jdk, error }).await;
}

const MAX_TEST_REPORT_FILES: usize = 256;
const MAX_TEST_REPORT_BYTES: u64 = 1024 * 1024;
const MAX_TEST_REPORT_TOTAL_BYTES: u64 = 16 * 1024 * 1024;
const MAX_TEST_FAILURES: usize = 1000;

fn collect_test_failures(project: &ProjectModel, started_at: SystemTime) -> Vec<TestFailure> {
    let mut modules = vec![project.root.clone()];
    modules.extend(project.modules.iter().map(|module| {
        if module.root.is_absolute() { module.root.clone() } else { project.root.join(&module.root) }
    }));
    modules.sort();
    modules.dedup();

    let mut report_files = Vec::new();
    for module in modules {
        match project.kind {
            ProjectKind::Maven => {
                for directory in ["target/surefire-reports", "target/failsafe-reports"] {
                    add_test_report_files(&module, &module.join(directory), started_at, &mut report_files);
                }
            }
            ProjectKind::Gradle => {
                let results = module.join("build/test-results");
                add_test_report_files(&module, &results, started_at, &mut report_files);
                if let Ok(entries) = std::fs::read_dir(&results) {
                    for entry in entries.flatten() {
                        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                            add_test_report_files(&module, &entry.path(), started_at, &mut report_files);
                        }
                    }
                }
            }
            ProjectKind::Unknown => return Vec::new(),
        }
        if report_files.len() >= MAX_TEST_REPORT_FILES { break; }
    }
    report_files.sort();
    report_files.dedup();

    let mut total_bytes = 0u64;
    let mut failures = Vec::new();
    for (module_root, path) in report_files.into_iter().take(MAX_TEST_REPORT_FILES) {
        let Ok(metadata) = std::fs::metadata(&path) else { continue; };
        if metadata.len() > MAX_TEST_REPORT_BYTES
            || total_bytes.saturating_add(metadata.len()) > MAX_TEST_REPORT_TOTAL_BYTES
        {
            continue;
        }
        total_bytes = total_bytes.saturating_add(metadata.len());
        let Ok(xml) = std::fs::read_to_string(path) else { continue; };
        let Ok(report) = quick_xml::de::from_str::<XmlTestReport>(&xml) else { continue; };
        append_test_failures(&report, None, &module_root, &mut failures);
        if failures.len() >= MAX_TEST_FAILURES { break; }
    }
    failures.truncate(MAX_TEST_FAILURES);
    failures
}

fn add_test_report_files(module_root: &Path, directory: &Path, started_at: SystemTime, output: &mut Vec<(PathBuf, PathBuf)>) {
    if output.len() >= MAX_TEST_REPORT_FILES { return; }
    let Ok(entries) = std::fs::read_dir(directory) else { return; };
    for entry in entries.flatten() {
        if output.len() >= MAX_TEST_REPORT_FILES { break; }
        if !entry.file_type().is_ok_and(|kind| kind.is_file())
            || entry.path().extension().is_none_or(|extension| extension != "xml")
        {
            continue;
        }
        let Ok(metadata) = entry.metadata() else { continue; };
        if metadata.len() <= MAX_TEST_REPORT_BYTES
            && metadata.modified().is_ok_and(|modified| modified >= started_at)
        {
            output.push((module_root.to_path_buf(), entry.path()));
        }
    }
}

fn append_test_failures(report: &XmlTestReport, inherited_class: Option<&str>, module_root: &Path, output: &mut Vec<TestFailure>) {
    for case in &report.cases {
        let class_name = case.class_name.as_deref().or(report.name.as_deref()).or(inherited_class)
            .unwrap_or("Unknown test class");
        for (kind, messages) in [("Failure", &case.failures), ("Error", &case.errors)] {
            for detail in messages {
                let message = detail.message.as_deref().or(detail.text.as_deref()).unwrap_or(kind);
                output.push(TestFailure {
                    class_name: class_name.chars().take(256).collect(),
                    test_name: case.name.as_deref().unwrap_or("Unknown test").chars().take(256).collect(),
                    message: message.chars().take(1024).collect(),
                    source_path: test_source_path(module_root, class_name),
                    source_line: detail.text.as_deref().and_then(|text| test_source_line(text, class_name)),
                });
                if output.len() >= MAX_TEST_FAILURES { return; }
            }
        }
    }
    for suite in &report.suites {
        append_test_failures(suite, report.name.as_deref().or(inherited_class), module_root, output);
        if output.len() >= MAX_TEST_FAILURES { return; }
    }
}

fn test_source_path(module_root: &Path, class_name: &str) -> Option<PathBuf> {
    if class_name.is_empty() || !class_name.chars().all(|ch| ch.is_alphanumeric() || matches!(ch, '.' | '_' | '$')) {
        return None;
    }
    let top_level = class_name.split('$').next()?;
    let relative = format!("{}.java", top_level.replace('.', "/"));
    ["src/test/java", "src/integrationTest/java", "src/it/java", "src/testFixtures/java"]
        .into_iter()
        .map(|source_root| module_root.join(source_root).join(&relative))
        .find(|path| path.is_file())
}

fn test_source_line(stack_trace: &str, class_name: &str) -> Option<usize> {
    let source_name = class_name.rsplit('.').next()?.split('$').next()?;
    let marker = format!("({source_name}.java:");
    stack_trace.lines().find_map(|line| {
        let start = line.find(&marker)? + marker.len();
        let digits: String = line[start..].chars().take_while(char::is_ascii_digit).collect();
        digits.parse::<usize>().ok().filter(|line| *line > 0)
    })
}

fn terminate_group(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid.filter(|pid| *pid > 1) {
        // SAFETY: this child was launched in a new process group with its own PID.
        unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL); }
    }
    #[cfg(not(unix))]
    let _ = pid;
}

async fn read_output(mut reader: impl AsyncRead + Unpin, stderr: bool, root: PathBuf, events: mpsc::Sender<BuildEvent>) {
    let mut chunk = [0u8; 4096];
    let mut pending = Vec::new();
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(count) => {
                pending.extend_from_slice(&chunk[..count]);
                while let Some(index) = pending.iter().position(|byte| *byte == b'\n') {
                    let line: Vec<_> = pending.drain(..=index).collect();
                    if !emit_output(&line, stderr, &root, &events).await { return; }
                }
                // Bound even malformed tool output with no line breaks.
                if pending.len() >= 16_384 {
                    let flush = utf8_safe_prefix_len(&pending);
                    if flush > 0 {
                        let output: Vec<_> = pending.drain(..flush).collect();
                        if !emit_output(&output, stderr, &root, &events).await { return; }
                    }
                }
            }
            Err(error) => {
                let _ = events.send(BuildEvent::Output { stderr: true, text: format!("Build stream error: {error}") }).await;
                break;
            }
        }
    }
    if !pending.is_empty() { let _ = emit_output(&pending, stderr, &root, &events).await; }
}

fn utf8_safe_prefix_len(bytes: &[u8]) -> usize {
    let Some(last) = bytes.len().checked_sub(1) else { return 0; };
    let mut start = last;
    while start > 0 && bytes[start] & 0b1100_0000 == 0b1000_0000 { start -= 1; }
    let expected = match bytes[start] {
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => 1,
    };
    if bytes.len() - start < expected { start } else { bytes.len() }
}

async fn emit_output(bytes: &[u8], stderr: bool, root: &Path, events: &mpsc::Sender<BuildEvent>) -> bool {
    let text = diagnostics::plain_text(&String::from_utf8_lossy(bytes)).trim_end_matches('\n').to_owned();
    let diagnostic = diagnostics::parse(root, &text);
    let tests = diagnostics::parse_test_summary(&text);
    if events.send(BuildEvent::Output { stderr, text }).await.is_err() { return false; }
    if let Some(diagnostic) = diagnostic {
        if events.send(BuildEvent::Diagnostic(diagnostic)).await.is_err() { return false; }
    }
    if let Some(summary) = tests {
        if events.send(BuildEvent::Tests(summary)).await.is_err() { return false; }
    }
    true
}

#[derive(Default)]
pub struct BuildTool {
    handle: Option<BuildHandle>,
    /// Cancelled projects keep their handles until the worker is joined. Their
    /// queued output is drained only for backpressure, never shown in this view.
    retiring: Vec<BuildHandle>,
    pub output: VecDeque<(bool, String)>,
    pub diagnostics: Vec<BuildDiagnostic>,
    pub test_summary: Option<diagnostics::TestSummary>,
    pub test_failures: Vec<TestFailure>,
    pub summary: String,
    pub selected_jdk: String,
    started: Option<Instant>,
    cancelling: bool,
    completion_received: bool,
    successful_completion: bool,
    origin: Option<PathBuf>,
}
impl BuildTool {
    pub fn running(&self) -> bool { self.handle.is_some() }
    pub fn has_workers(&self) -> bool { self.running() || !self.retiring.is_empty() }
    pub fn take_successful_completion(&mut self) -> bool {
        std::mem::take(&mut self.successful_completion)
    }
    pub fn reset(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.cancel();
            self.retiring.push(handle);
        }
        self.output.clear();
        self.diagnostics.clear();
        self.test_summary = None;
        self.test_failures.clear();
        self.summary.clear();
        self.selected_jdk.clear();
        self.started = None;
        self.cancelling = false;
        self.completion_received = false;
        self.successful_completion = false;
        self.origin = None;
    }
    pub fn start(&mut self, request: BuildRequest, executable: Option<&Path>) -> Result<(), BuildError> {
        if self.running() { return Err(BuildError("A build is already running".into())); }
        let label = format!("{} — {}", request.jdk.label(), request.jdk.home.display());
        let origin = request.project.root.clone();
        let summary = format!("{} {} in {}", request.project.kind, request.task, request.project.root.display());
        let handle = if executable.is_some() { BuildService::start_with_executable(request, executable)? }
            else { BuildService::start(request)? };
        self.install_handle(handle, origin, label, summary);
        Ok(())
    }
    pub fn start_maven_task(&mut self, request: MavenTaskRequest, executable: Option<&Path>) -> Result<(), BuildError> {
        if self.running() { return Err(BuildError("A build is already running".into())); }
        let label = format!("{} — {}", request.jdk.label(), request.jdk.home.display());
        let origin = request.project.root.clone();
        let summary = format!("Maven {} in {}", request.goal, request.project.root.display());
        let handle = BuildService::start_maven_task(request, executable)?;
        self.install_handle(handle, origin, label, summary);
        Ok(())
    }
    pub fn start_gradle_task(&mut self, request: GradleTaskRequest, executable: Option<&Path>) -> Result<(), BuildError> {
        if self.running() { return Err(BuildError("A build is already running".into())); }
        let label = format!("{} — {}", request.jdk.label(), request.jdk.home.display());
        let origin = request.project.root.clone();
        let summary = format!("Gradle {} in {}", request.task_path, request.project.root.display());
        let handle = BuildService::start_gradle_task(request, executable)?;
        self.install_handle(handle, origin, label, summary);
        Ok(())
    }
    fn install_handle(&mut self, handle: BuildHandle, origin: PathBuf, label: String, summary: String) {
        self.output.clear();
        self.diagnostics.clear();
        self.test_summary = None;
        self.test_failures.clear();
        self.summary = summary;
        self.selected_jdk = label;
        self.handle = Some(handle);
        self.started = Some(Instant::now());
        self.cancelling = false;
        self.completion_received = false;
        self.successful_completion = false;
        self.origin = Some(origin);
    }
    pub fn cancel(&mut self) {
        if let Some(handle) = &self.handle { handle.cancel(); self.cancelling = true; }
        for handle in &self.retiring { handle.cancel(); }
    }
    pub fn poll(&mut self) {
        self.retiring.retain_mut(|handle| {
            // Receivers must remain open and drained until cancellation/reaping
            // and the final event send have completed on the worker.
            for _ in 0..256 {
                if handle.try_event().is_err() { break; }
            }
            handle.poll_worker_completion().is_none()
        });
        let Some(handle) = &mut self.handle else { return; };
        let mut events = Vec::new();
        for _ in 0..256 {
            match handle.try_event() { Ok(event) => events.push(event), Err(_) => break }
        }
        let completion = handle.poll_worker_completion();
        if completion.is_some() {
            // Joining establishes that no producer can append another event.
            // The bounded channel may still contain its final diagnostics/status.
            while let Ok(event) = handle.try_event() { events.push(event); }
        }
        for event in events {
            match event {
                BuildEvent::Output { stderr, text } => {
                    self.output.push_back((stderr, text));
                    if self.output.len() > 4000 { self.output.pop_front(); }
                }
                BuildEvent::Diagnostic(diagnostic) => {
                    if self.diagnostics.len() < 2000 && !self.diagnostics.contains(&diagnostic) { self.diagnostics.push(diagnostic); }
                }
                BuildEvent::Tests(summary) => {
                    let total = self.test_summary.get_or_insert_with(diagnostics::TestSummary::default);
                    total.total = total.total.saturating_add(summary.total);
                    total.passed = total.passed.saturating_add(summary.passed);
                    total.failed = total.failed.saturating_add(summary.failed);
                    total.skipped = total.skipped.saturating_add(summary.skipped);
                }
                BuildEvent::TestFailures(failures) => {
                    for failure in failures {
                        if self.test_failures.len() >= MAX_TEST_FAILURES { break; }
                        self.test_failures.push(failure);
                    }
                }
                BuildEvent::Finished { exit_code, elapsed, cancelled, jdk, error } => {
                    self.successful_completion = exit_code == Some(0) && !cancelled && error.is_none();
                    self.selected_jdk = jdk;
                    let summary = if let Some(error) = error { format!("Failed after {:.1}s: {error}", elapsed.as_secs_f32()) }
                        else { format!("{} — exit {} — {:.1}s", if cancelled { "Cancelled" } else { "Finished" },
                            exit_code.map(|code| code.to_string()).unwrap_or_else(|| "signal / unavailable".into()), elapsed.as_secs_f32()) };
                    self.summary = format!("{} — {summary}", self.origin.as_deref().unwrap_or(Path::new("")).display());
                    self.completion_received = true;
                }
            }
        }
        if let Some(result) = completion {
            if result.is_ok() && !self.completion_received {
                self.summary = format!("{} — Build worker disconnected unexpectedly", self.origin.as_deref().unwrap_or(Path::new("")).display());
            }
            if let Err(error) = result {
                self.summary = format!("{} — {error}", self.origin.as_deref().unwrap_or(Path::new("")).display());
            }
            self.handle = None;
            self.started = None;
            self.cancelling = false;
        }
    }
}

pub fn render(ui: &mut egui::Ui, state: &mut crate::app::AppState) {
    use crate::app::events::{AppEvent, BuildAction};
    let mut action = None;
    let available = state.project.as_ref().and_then(|project| project.model.as_ref()).is_some();
    ui.horizontal(|ui| {
        for (label, intent) in [("Build", BuildAction::Build), ("Test", BuildAction::Test), ("Clean", BuildAction::Clean)] {
            if ui.add_enabled(available && !state.build.running(), egui::Button::new(label)).clicked() { action = Some(intent); }
        }
        if ui.add_enabled(state.build.running() && !state.build.cancelling, egui::Button::new("Cancel")).clicked() { action = Some(BuildAction::Cancel); }
        if let Some(start) = state.build.started { ui.label(format!("{} {:.1}s", if state.build.cancelling { "Cancelling…" } else { "Running…" }, start.elapsed().as_secs_f32())); }
    });
    ui.label(&state.build.summary);
    if !state.build.selected_jdk.is_empty() { ui.small(format!("JDK: {}", state.build.selected_jdk)); }
    if let Some(summary) = state.build.test_summary {
        let color = if summary.failed > 0 {
            ui.visuals().error_fg_color
        } else {
            ui.visuals().text_color()
        };
        ui.colored_label(
            color,
            format!(
                "Tests reported: {} total · {} passed · {} failed · {} skipped",
                summary.total, summary.passed, summary.failed, summary.skipped
            ),
        );
    }
    let mut navigation = None;
    if !state.build.test_failures.is_empty() {
        egui::CollapsingHeader::new(format!("Failed tests ({})", state.build.test_failures.len()))
            .default_open(true)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("build_test_failures")
                    .max_height(120.0)
                    .show(ui, |ui| {
                        for failure in &state.build.test_failures {
                            let label = format!("{}.{}", failure.class_name, failure.test_name);
                            if let Some(path) = &failure.source_path {
                                if ui.selectable_label(false, label).clicked() {
                                    navigation = Some((path.clone(), failure.source_line.unwrap_or(1)));
                                }
                            } else {
                                ui.colored_label(ui.visuals().error_fg_color, label);
                            }
                            if !failure.message.is_empty() {
                                ui.small(&failure.message);
                            }
                        }
                    });
            });
    }
    if !state.build.diagnostics.is_empty() {
        egui::CollapsingHeader::new(format!("Compiler diagnostics ({})", state.build.diagnostics.len())).default_open(true).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("build_diagnostics").max_height(90.0).show(ui, |ui| {
                for diagnostic in &state.build.diagnostics {
                    let path = diagnostic.path.as_ref().map(|path| path.display().to_string()).unwrap_or_default();
                    let label = format!("{:?} {}:{} {}", diagnostic.severity, path, diagnostic.line.unwrap_or(1), diagnostic.message);
                    if ui.selectable_label(false, label).clicked() {
                        if let Some(path) = &diagnostic.path { navigation = Some((path.clone(), diagnostic.line.unwrap_or(1) as usize)); }
                    }
                }
            });
        });
    }
    egui::ScrollArea::both().id_salt("build_output").stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
        for (stderr, text) in &state.build.output {
            let text = egui::RichText::new(text).monospace();
            ui.label(if *stderr { text.color(ui.visuals().warn_fg_color) } else { text });
        }
    });
    if state.build.running() { ui.ctx().request_repaint_after(Duration::from_millis(40)); }
    if let Some(action) = action { state.enqueue(AppEvent::Build(action)); }
    if let Some((path, line)) = navigation { state.enqueue(AppEvent::OpenFileAtLine(path, line)); }
}
