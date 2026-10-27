//! A Java debug session owns a worker, DAP connection and cancellation token.
//! JDT LS owns the adapter and the adapter owns/reaps its launched debuggee.
use std::path::PathBuf;
use std::collections::BTreeSet;
use std::thread::JoinHandle;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};

use crate::java::jdk::JdkInstallation;
use crate::java::lsp::DebugAdapterResolver;
use crate::java::run_config::RunConfiguration;
use crate::project::ProjectModel;
use super::DebugError;
use super::stack::{DebugController, DebugInspector, StopEpoch, InspectRequest};
use super::breakpoints::{BreakpointStore, BreakpointStoreEvent, Breakpoints};
use super::dap::{DapClient, DapConnection, DapEvent};

#[derive(Clone, Debug)]
pub enum DebugState {
    Starting,
    Running,
    Stopped,
    Disconnecting,
    Terminated { exit_code: Option<i64>, cancelled: bool },
    Failed(DebugError),
}
impl Default for DebugState { fn default() -> Self { Self::Terminated { exit_code: None, cancelled: false } } }
#[derive(Clone, Debug)]
pub enum DebugEvent {
    Output { category: String, text: String },
    Initialized,
    Ready,
    Stopped { thread_id: Option<i64>, reason: String, all_threads_stopped: bool },
    Continued { thread_id: Option<i64>, all_threads_continued: bool },
    Thread { thread_id: i64, reason: String },
    Breakpoint { reason: String, breakpoint: Value },
    Exited { code: i64 },
    Terminated { exit_code: Option<i64>, cancelled: bool },
    Failed(DebugError),
    AdapterEvent { event: String, body: Value },
}

/// Can be cloned into UI request workers. A client is scoped to exactly one
/// session; pending/old callers never attach to a replacement debug session.
#[derive(Clone)]
pub struct DebugClient { ready: watch::Receiver<Option<DapClient>>, cancel: watch::Receiver<bool>, resumes: mpsc::UnboundedSender<(u64, i64)> }
impl DebugClient {
    pub(super) fn resumed(&self, epoch: u64, thread: i64) -> Result<(), DebugError> {
        self.resumes.send((epoch, thread)).map_err(|_| DebugError::Cancelled)
    }
    pub async fn request(&self, command: &str, arguments: Value) -> Result<Value, DebugError> {
        self.request_checked(command, arguments, || Ok(())).await
    }
    pub(super) async fn request_checked(&self, command: &str, arguments: Value, check: impl Fn() -> Result<(), DebugError>) -> Result<Value, DebugError> {
        let mut ready = self.ready.clone();
        let mut cancel = self.cancel.clone();
        let client = loop {
            if *cancel.borrow() { return Err(DebugError::Cancelled); }
            if let Some(client) = ready.borrow().clone() { break client; }
            tokio::select! {
                _ = cancel.changed() => return Err(DebugError::Cancelled),
                result = ready.changed() => if result.is_err() { return Err(DebugError::Transport("Debug session ended before it was ready".into())); },
            }
        };
        check()?;
        if *cancel.borrow() { return Err(DebugError::Cancelled); }
        tokio::select! {
            _ = cancel.changed() => Err(DebugError::Cancelled),
            result = client.request(command, arguments) => result,
        }
    }
}

pub struct DebugSession {
    events: mpsc::Receiver<DebugEvent>,
    state: watch::Receiver<DebugState>,
    cancellation: watch::Sender<bool>,
    client: DebugClient,
    worker: Option<JoinHandle<()>>,
    host_restart: Arc<AtomicBool>,
    stop_epoch: StopEpoch,
}
impl DebugSession {
    pub fn client(&self) -> DebugClient { self.client.clone() }
    pub fn controller(&self) -> DebugController { DebugController::new(self.client(), self.stop_epoch.clone()) }
    pub fn state(&self) -> DebugState { self.state.borrow().clone() }
    pub fn cancel(&self) { self.stop_epoch.resumed(); let _ = self.cancellation.send(true); }
    pub fn needs_host_restart(&self) -> bool { self.host_restart.load(Ordering::Acquire) }
    pub fn poll(&mut self) -> Vec<DebugEvent> {
        let mut events = Vec::new();
        for _ in 0..256 { match self.events.try_recv() { Ok(event) => events.push(event), Err(_) => break } }
        events
    }
    /// Joining only a finished thread keeps the egui thread responsive.
    pub fn reap(&mut self) -> Option<Result<(), DebugError>> {
        if !self.worker.as_ref().is_some_and(JoinHandle::is_finished) { return None; }
        Some(self.worker.take().unwrap().join().map_err(|_| DebugError::Transport("Debug session worker panicked".into())))
    }
}
impl Drop for DebugSession { fn drop(&mut self) { self.cancel(); } }

pub struct DebugService {
    resolver: DebugAdapterResolver,
    bundle: PathBuf,
    initial_requests: Vec<(String, Value)>,
}
impl DebugService {
    pub fn new(resolver: DebugAdapterResolver, bundle: PathBuf) -> Self {
        Self { resolver, bundle, initial_requests: Vec::new() }
    }
    /// setBreakpoints / setExceptionBreakpoints can run before configurationDone.
    /// The launch response may be deferred until these configuration requests finish.
    pub fn with_initial_requests(mut self, requests: Vec<(String, Value)>) -> Self {
        self.initial_requests = requests; self
    }
    pub fn start(&self, config: &RunConfiguration, project: &ProjectModel, jdk: &JdkInstallation) -> Result<DebugSession, DebugError> {
        config.validate().map_err(|error| DebugError::Configuration(error.to_string()))?;
        let host_restart = Arc::new(AtomicBool::new(false));
        let worker_host_restart = host_restart.clone();
        let (events, receiver) = mpsc::channel(256);
        let (state, state_receiver) = watch::channel(DebugState::Starting);
        let (cancellation, cancel) = watch::channel(false);
        let (ready, ready_receiver) = watch::channel(None);
        let (resumes, resume_receiver) = mpsc::unbounded_channel();
        let client = DebugClient { ready: ready_receiver, cancel: cancel.clone(), resumes };
        let stop_epoch = StopEpoch::default();
        let worker_stop_epoch = stop_epoch.clone();
        let config = config.clone(); let project = project.clone(); let jdk = jdk.clone();
        let bundle_path = self.bundle.clone();
        let resolver = self.resolver.clone(); let initial_requests = self.initial_requests.clone();
        let worker = std::thread::Builder::new().name("java-debug".into()).spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build();
            match runtime {
                Ok(runtime) => runtime.block_on(async {
                    let mut context = EventContext { events, state, cancel, exit_code: None, initialized: false, terminated: false, stopped: false, stop_epoch: worker_stop_epoch, resumes: resume_receiver };
                    let setup = (|| {
                        let bundle = super::validate_bundle(&bundle_path)?;
                        resolver.validate(&project.root, &bundle).map_err(|error| DebugError::Configuration(error.to_string()))
                    })().and_then(|()| {
                        let cwd = config.working_directory_at(&project.root);
                        if !cwd.is_dir() {
                            return Err(DebugError::Configuration(format!("Working directory does not exist: {}", cwd.display())));
                        }
                        let environment = jdk.child_environment().map_err(|error| DebugError::Configuration(error.to_string()))?;
                        let mut env = std::collections::BTreeMap::<String, String>::new();
                        for (name, value) in environment {
                            let name = name.into_string().map_err(|_| DebugError::Configuration("JDK environment key is not UTF-8".into()))?;
                            let value = value.into_string().map_err(|_| DebugError::Configuration("JDK environment value is not UTF-8".into()))?;
                            env.insert(name, value);
                        }
                        env.extend(config.environment.clone());
                        Ok(env)
                    });
                    let result = match setup {
                        Ok(env) => run(config, project, jdk, env, resolver, initial_requests, ready, worker_host_restart, &mut context).await,
                        Err(error) => Err(error),
                    };
                    match result {
                        Err(DebugError::Cancelled) => context.finish(true),
                        Err(error) => {
                            context.state.send_replace(DebugState::Failed(error.clone()));
                            let _ = context.events.try_send(DebugEvent::Failed(error));
                        }
                        Ok(()) => { let cancelled = *context.cancel.borrow(); context.finish(cancelled); },
                    }
                }),
                Err(error) => {
                    let error = DebugError::Transport(format!("Cannot create debug runtime: {error}"));
                    state.send_replace(DebugState::Failed(error.clone()));
                    let _ = events.try_send(DebugEvent::Failed(error));
                }
            }
        }).map_err(|error| DebugError::Transport(format!("Cannot start debug worker: {error}")))?;
        Ok(DebugSession { events: receiver, state: state_receiver, cancellation, client, worker: Some(worker), host_restart, stop_epoch })
    }
}
struct EventContext {
    events: mpsc::Sender<DebugEvent>,
    state: watch::Sender<DebugState>,
    cancel: watch::Receiver<bool>,
    exit_code: Option<i64>,
    initialized: bool,
    terminated: bool,
    stopped: bool,
    stop_epoch: StopEpoch,
    resumes: mpsc::UnboundedReceiver<(u64, i64)>,
}
impl EventContext {
    async fn emit(&mut self, event: DebugEvent) {
        if *self.cancel.borrow() { let _ = self.events.try_send(event); return; }
        tokio::select! {
            _ = self.cancel.changed() => {},
            _ = self.events.send(event) => {},
        }
    }
    async fn acknowledge_resume(&mut self, epoch: u64, thread: i64) {
        // This worker serializes acknowledgements with adapter events. Only
        // the exact stop that issued the successful request may become running.
        if !*self.cancel.borrow() && !self.terminated && self.exit_code.is_none() && self.stop_epoch.resume_if(epoch) {
            self.stopped = false;
            self.state.send_replace(DebugState::Running);
            self.emit(DebugEvent::Continued { thread_id: Some(thread), all_threads_continued: true }).await;
        }
    }
    async fn adapter_event(&mut self, event: DapEvent) {
        let body = event.body;
        let event = match event.event.as_str() {
            "initialized" => { self.initialized = true; DebugEvent::Initialized },
            "output" => DebugEvent::Output { category: body["category"].as_str().unwrap_or("console").into(), text: body["output"].as_str().unwrap_or_default().into() },
            "stopped" => {
                self.stop_epoch.stopped();
                self.stopped = true; self.state.send_replace(DebugState::Stopped);
                DebugEvent::Stopped { thread_id: body["threadId"].as_i64(), reason: body["reason"].as_str().unwrap_or_default().into(),
                    all_threads_stopped: body["allThreadsStopped"].as_bool().unwrap_or(false) }
            }
            "continued" => {
                self.stop_epoch.resumed();
                self.stopped = false; self.state.send_replace(DebugState::Running);
                DebugEvent::Continued { thread_id: body["threadId"].as_i64(), all_threads_continued: body["allThreadsContinued"].as_bool().unwrap_or(true) }
            }
            "thread" => {
                if self.stopped { self.stop_epoch.stopped(); }
                DebugEvent::Thread { thread_id: body["threadId"].as_i64().unwrap_or_default(), reason: body["reason"].as_str().unwrap_or_default().into() }
            },
            "breakpoint" => DebugEvent::Breakpoint { reason: body["reason"].as_str().unwrap_or("changed").into(), breakpoint: body["breakpoint"].clone() },
            "exited" => {
                self.stop_epoch.resumed();
                let code = body["exitCode"].as_i64().unwrap_or_default(); self.exit_code = Some(code);
                DebugEvent::Exited { code }
            }
            "terminated" => { self.stop_epoch.resumed(); self.terminated = true; DebugEvent::AdapterEvent { event: event.event, body } },
            _ => DebugEvent::AdapterEvent { event: event.event, body },
        };
        self.emit(event).await;
    }
    fn finish(&mut self, cancelled: bool) {
        self.stop_epoch.resumed();
        self.state.send_replace(DebugState::Terminated { exit_code: self.exit_code, cancelled });
        let _ = self.events.try_send(DebugEvent::Terminated { exit_code: self.exit_code, cancelled });
    }
}

/// Java Debug Server's non-Windows parser consumes backslash escapes outside
/// quotes, and consumes \\ / \" escapes inside double quotes. Quote every
/// entry and escape every backslash / double quote; no shell is invoked.
fn argument_string(arguments: &[String]) -> Result<String, DebugError> {
    arguments.iter().map(|argument| {
        if argument.contains('\0') {
            return Err(DebugError::Configuration("Debug arguments cannot contain NUL characters".into()));
        }
        let mut result = String::from("\"");
        for ch in argument.chars() {
            if ch == '\\' || ch == '"' { result.push('\\'); }
            result.push(ch);
        }
        result.push('"');
        Ok(result)
    }).collect::<Result<Vec<_>, _>>().map(|arguments| arguments.join(" "))
}

async fn run(config: RunConfiguration, project: ProjectModel, jdk: JdkInstallation,
    env: std::collections::BTreeMap<String, String>, resolver: DebugAdapterResolver,
    initial_requests: Vec<(String, Value)>, ready: watch::Sender<Option<DapClient>>, host_restart: Arc<AtomicBool>, context: &mut EventContext) -> Result<(), DebugError> {
    if *context.cancel.borrow() { return Err(DebugError::Cancelled); }
    let program_arguments = argument_string(&config.program_args)?;
    let vm_arguments = argument_string(&config.vm_args)?;
    let paths = tokio::select! {
        _ = context.cancel.changed() => return Err(DebugError::Cancelled),
        result = resolver.resolve_classpaths(&project, &config.main_class, config.project_module.as_deref()) => result.map_err(|error| DebugError::LanguageService(error.to_string()))?,
    };
    host_restart.store(true, Ordering::Release);
    let port = tokio::select! {
        _ = context.cancel.changed() => return Err(DebugError::Cancelled),
        result = resolver.start_debug_session() => result.map_err(|error| DebugError::LanguageService(error.to_string()))?,
    };
    // Until connected, only the owning JDT LS process can dispose of a port
    // allocated concurrently with cancellation or a connection failure.
    host_restart.store(true, Ordering::Release);
    if *context.cancel.borrow() { return Err(DebugError::Cancelled); }
    let mut connection = tokio::select! {
        _ = context.cancel.changed() => return Err(DebugError::Cancelled),
        result = DapConnection::connect(port) => result?,
    };
    let main = config.module.as_ref().map(|module| format!("{module}/{}", config.main_class)).unwrap_or_else(|| config.main_class.clone());
    let arguments = json!({
        "name": config.name, "type": "java", "request": "launch", "mainClass": main,
        "projectName": paths.project_name.as_deref().unwrap_or(&project.name),
        "javaExec": jdk.home.join("bin/java"), "cwd": config.working_directory_at(&project.root),
        "args": program_arguments, "vmArgs": vm_arguments,
        "env": env, "classPaths": paths.classpaths, "modulePaths": paths.modulepaths,
        "console": "internalConsole", "stopOnEntry": false, "noDebug": false,
    });
    let result = active(&mut connection, arguments, initial_requests, &ready, context).await;
    // Always request debuggee termination, including initialize/launch failure.
    // Continue draining output and exited events while disconnect is pending.
    context.stop_epoch.resumed();
    context.state.send_replace(DebugState::Disconnecting);
    let cleanup = disconnect(&mut connection, context).await;
    if cleanup.is_ok() { host_restart.store(false, Ordering::Release); }
    ready.send_replace(None);
    connection.close().await;
    match (result, cleanup) {
        (Err(DebugError::Cancelled), Err(error)) => Err(error),
        (Err(error), _) => Err(error),
        (Ok(()), cleanup) => cleanup,
    }
}

async fn request(connection: &mut DapConnection, context: &mut EventContext, command: &str, arguments: Value, timeout: Duration) -> Result<Value, DebugError> {
    if *context.cancel.borrow() { return Err(DebugError::Cancelled); }
    let client = connection.client();
    let request = client.request_timeout(command, arguments, timeout); tokio::pin!(request);
    loop {
        tokio::select! {
            biased;
            _ = context.cancel.changed() => return Err(DebugError::Cancelled),
            result = &mut request => return result,
            event = connection.next_event() => match event {
                Some(Ok(event)) => {
                    context.adapter_event(event).await;
                    if context.terminated || context.exit_code.is_some() { return Err(DebugError::ApplicationExit(context.exit_code)); }
                }
                Some(Err(error)) => return Err(error),
                None => return Err(DebugError::Transport(format!("DAP disconnected during {command}"))),
            }
        }
    }
}
async fn active(connection: &mut DapConnection, arguments: Value, initial_requests: Vec<(String, Value)>, ready: &watch::Sender<Option<DapClient>>, context: &mut EventContext) -> Result<(), DebugError> {
    let capabilities = request(connection, context, "initialize", json!({
        "clientID": "intelli-gen", "clientName": "Intelli-gen", "adapterID": "java",
        "locale": "en", "linesStartAt1": true, "columnsStartAt1": true, "pathFormat": "path",
        "supportsVariableType": true, "supportsVariablePaging": true, "supportsRunInTerminalRequest": false,
        "supportsStartDebuggingRequest": false,
    }), Duration::from_secs(20)).await?;
    let client = connection.client();
    // Launch can respond only after configurationDone. Drive it concurrently.
    let launch = client.start_request("launch", arguments, Duration::from_secs(90))?.wait(); tokio::pin!(launch);
    let initialized_deadline = tokio::time::sleep(Duration::from_secs(30)); tokio::pin!(initialized_deadline);
    let mut launch_result = None;
    while !context.initialized {
        tokio::select! {
            _ = context.cancel.changed() => return Err(DebugError::Cancelled),
            _ = &mut initialized_deadline => return Err(DebugError::Transport("Java debug adapter did not send initialized".into())),
            result = &mut launch, if launch_result.is_none() => {
                result?; launch_result = Some(());
            }
            event = connection.next_event() => match event {
                Some(Ok(event)) => {
                    context.adapter_event(event).await;
                    if context.terminated || context.exit_code.is_some() { return Err(DebugError::ApplicationExit(context.exit_code)); }
                }
                Some(Err(error)) => return Err(error),
                None => return Err(DebugError::Transport("DAP disconnected before initialized".into())),
            }
        }
    }
    for (command, arguments) in initial_requests {
        request(connection, context, &command, arguments, Duration::from_secs(20)).await?;
    }
    if capabilities["supportsConfigurationDoneRequest"].as_bool().unwrap_or(false) {
        request(connection, context, "configurationDone", json!({}), Duration::from_secs(20)).await?;
    }
    if launch_result.is_none() {
        loop {
            tokio::select! {
                _ = context.cancel.changed() => return Err(DebugError::Cancelled),
                result = &mut launch => { result?; break; },
                event = connection.next_event() => match event {
                    Some(Ok(event)) => {
                        context.adapter_event(event).await;
                        if context.terminated || context.exit_code.is_some() { return Err(DebugError::ApplicationExit(context.exit_code)); }
                    }
                    Some(Err(error)) => return Err(error),
                    None => return Err(DebugError::Transport("DAP disconnected during launch".into())),
                }
            }
        }
    }
    if context.terminated || context.exit_code.is_some() { return Err(DebugError::ApplicationExit(context.exit_code)); }
    ready.send_replace(Some(client.clone()));
    context.state.send_replace(if context.stopped { DebugState::Stopped } else { DebugState::Running });
    context.emit(DebugEvent::Ready).await;
    let mut exit_deadline = None;
    loop {
        if *context.cancel.borrow() { return Err(DebugError::Cancelled); }
        tokio::select! {
            _ = context.cancel.changed() => return Err(DebugError::Cancelled),
            _ = tokio::time::sleep_until(exit_deadline.unwrap_or_else(|| Instant::now() + Duration::from_secs(86_400)).into()), if exit_deadline.is_some() => return Ok(()),
            Some((epoch, thread)) = context.resumes.recv() => context.acknowledge_resume(epoch, thread).await,
            event = connection.next_event() => match event {
                Some(Ok(event)) => {
                    context.adapter_event(event).await;
                    if context.terminated { return Ok(()); }
                    // exited is distinct from adapter termination. Keep draining
                    // briefly even if an adapter omits its final terminated event.
                    if context.exit_code.is_some() && exit_deadline.is_none() { exit_deadline = Some(Instant::now() + Duration::from_secs(2)); }
                }
                Some(Err(error)) => return if context.exit_code.is_some() { Ok(()) } else { Err(error) },
                None => return if context.exit_code.is_some() { Ok(()) } else { Err(DebugError::Transport("Java debug server disconnected unexpectedly".into())) },
            }
        }
    }
}

async fn disconnect(connection: &mut DapConnection, context: &mut EventContext) -> Result<(), DebugError> {
    let client = connection.client();
    let request = client.request_timeout("disconnect", json!({"restart": false, "terminateDebuggee": true, "suspendDebuggee": false}), Duration::from_secs(3));
    tokio::pin!(request);
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut response = None;
    // Even when terminated arrives first, retain receiver for a brief final
    // output/exit drain. The adapter host is left to its owning JDT LS worker.
    let mut quiet_until = None;
    loop {
        let until = quiet_until.unwrap_or(deadline).min(deadline);
        tokio::select! {
            _ = tokio::time::sleep_until(until.into()) => break,
            result = &mut request, if response.is_none() => {
                response = Some(result);
                quiet_until = Some(Instant::now() + Duration::from_millis(250));
            }
            event = connection.next_event() => match event {
                Some(Ok(event)) => {
                    context.adapter_event(event).await;
                    if response.is_some() { quiet_until = Some(Instant::now() + Duration::from_millis(250)); }
                }
                Some(Err(error)) => return if context.terminated || context.exit_code.is_some() { Ok(()) } else { Err(error) },
                None => break,
            }
        }
    }
    match response {
        Some(Ok(_)) => Ok(()),
        Some(Err(_)) if context.terminated || context.exit_code.is_some() => Ok(()),
        Some(Err(error)) => Err(error),
        None if context.terminated || context.exit_code.is_some() => Ok(()),
        None => Err(DebugError::Transport("Java debug adapter did not acknowledge disconnect".into())),
    }
}

#[derive(Default)]
pub struct DebugTool {
    pub active: Option<DebugSession>,
    retiring: Vec<DebugSession>,
    pub state: DebugState,
    pub summary: String,
    /// Snapshot of the JDK used for this launch, retained after termination.
    pub selected_jdk: String,
    pub output: String,
    pub event_log: Vec<String>,
    pub last_events: Vec<DebugEvent>,
    pub stop_reason: Option<String>,
    pub host_restart_required: bool,
    pub breakpoints: Breakpoints,
    breakpoint_store: BreakpointStore,
    breakpoint_root: Option<PathBuf>,
    breakpoint_generation: u64,
    breakpoint_loaded: bool,
    breakpoint_touched: BTreeSet<PathBuf>,
    pub breakpoint_storage_error: Option<String>,
    pub inspector: DebugInspector,
    pub watches: Vec<String>,
    pub watch_input: String,
    pub(crate) watched_frame: Option<(u64, i64)>,
}
impl DebugTool {
    pub fn has_workers(&self) -> bool { self.active.is_some() || !self.retiring.is_empty() || self.inspector.has_workers() || self.breakpoint_store.has_pending() }
    pub fn controller(&self) -> Option<DebugController> { self.active.as_ref().map(DebugSession::controller) }
    pub fn client(&self) -> Option<DebugClient> { self.active.as_ref().map(DebugSession::client) }
    pub fn cancel(&self) {
        if let Some(session) = &self.active { session.cancel(); }
        for session in &self.retiring { session.cancel(); }
    }
    pub fn open_project(&mut self, root: PathBuf) {
        self.cancel();
        self.inspector.detach();
        self.breakpoints = Breakpoints::default();
        self.breakpoint_root = Some(root.clone());
        self.breakpoint_generation = self.breakpoint_generation.wrapping_add(1);
        self.breakpoint_loaded = false;
        self.breakpoint_touched.clear();
        self.breakpoint_storage_error = None;
        if let Err(error) = self.breakpoint_store.load(root, self.breakpoint_generation) {
            self.breakpoint_storage_error = Some(error);
        }
        self.watches.clear();
        self.watch_input.clear();
        self.watched_frame = None;
        if let Some(session) = self.active.take() { self.retiring.push(session); }
        self.state = DebugState::default(); self.summary.clear(); self.selected_jdk.clear(); self.output.clear(); self.event_log.clear(); self.last_events.clear(); self.stop_reason = None;
    }
    pub fn mark_breakpoint_changed(&mut self, source: &std::path::Path) {
        let Some(root) = self.breakpoint_root.as_ref() else { return; };
        if !source.starts_with(root) { return; }
        self.breakpoint_touched.insert(source.to_path_buf());
        if self.breakpoint_loaded { self.save_breakpoints(); }
    }
    pub fn rename_breakpoint_source(&mut self, old: &std::path::Path, new: PathBuf) {
        self.breakpoint_touched.insert(old.to_path_buf());
        self.breakpoint_touched.insert(new.clone());
        let had_breakpoints = self.breakpoints.rename_source(old, new.clone());
        if had_breakpoints {
            if let Some(controller) = self.controller() {
                self.inspector.submit(controller.clone(), InspectRequest::Breakpoints {
                    source: old.to_path_buf(),
                    revision: self.breakpoints.source_revision(old),
                    arguments: json!({"source":{"path":old,"name":old.file_name().map(|name|name.to_string_lossy())},"breakpoints":[],"sourceModified":false}),
                });
                self.inspector.submit(controller, InspectRequest::Breakpoints {
                    source: new.clone(),
                    revision: self.breakpoints.source_revision(&new),
                    arguments: self.breakpoints.arguments(&new),
                });
            }
        }
        if self.breakpoint_loaded { self.save_breakpoints(); }
    }
    fn save_breakpoints(&mut self) {
        let Some(root) = self.breakpoint_root.clone() else { return; };
        if let Err(error) = self.breakpoint_store.save(root, self.breakpoint_generation, self.breakpoints.snapshot()) {
            self.breakpoint_storage_error = Some(error);
        }
    }
    fn poll_breakpoint_store(&mut self) {
        for event in self.breakpoint_store.poll() {
            match event {
                BreakpointStoreEvent::Loaded { root, generation, result }
                    if self.breakpoint_root.as_ref() == Some(&root) && generation == self.breakpoint_generation => {
                    match result {
                        Ok(points) => {
                            self.breakpoints.restore(points, &self.breakpoint_touched);
                            self.breakpoint_loaded = true;
                            self.breakpoint_storage_error = None;
                            if !self.breakpoint_touched.is_empty() { self.save_breakpoints(); }
                        }
                        Err(error) => self.breakpoint_storage_error = Some(error),
                    }
                }
                BreakpointStoreEvent::Saved { root, generation, result }
                    if self.breakpoint_root.as_ref() == Some(&root) && generation == self.breakpoint_generation => {
                    self.breakpoint_storage_error = result.err();
                }
                _ => {}
            }
        }
    }
    pub fn add_watch(&mut self, expr: String) {
        if !self.watches.contains(&expr) { self.watches.push(expr); }
    }
    pub fn remove_watch(&mut self, index: usize) {
        if index < self.watches.len() { self.watches.remove(index); }
    }
    pub fn start(&mut self, service: DebugService, config: &RunConfiguration, project: &ProjectModel, jdk: &JdkInstallation) -> Result<(), DebugError> {
        if self.has_workers() { return Err(DebugError::Configuration("Wait for the previous debug session to finish stopping".into())); }
        self.selected_jdk = format!("{} — {}", jdk.label(), jdk.home.display());
        self.breakpoints.clear_adapter_state();
        self.inspector.attach()?;
        let initial_sources:Vec<_>=self.breakpoints.sources.keys().cloned().collect();
        for source in initial_sources {self.breakpoints.begin_sync(&source,self.breakpoints.source_revision(&source));}
        match service.with_initial_requests(self.breakpoints.initial_requests()).start(config, project, jdk) {
            Ok(session) => self.active = Some(session),
            Err(error) => { self.inspector.detach(); self.breakpoints.clear_retained_events(); return Err(error); }
        }
        self.state = DebugState::Starting; self.summary = "Starting Java debugger…".into(); self.output.clear(); self.event_log.clear(); self.last_events.clear(); self.stop_reason = None;
        Ok(())
    }
    pub fn poll(&mut self) {
        self.poll_breakpoint_store();
        self.last_events.clear();
        let mut host_restart_required = false;
        self.retiring.retain_mut(|session| {
            session.poll();
            let done = session.reap();
            if done.is_some() { host_restart_required |= session.needs_host_restart(); }
            done.is_none()
        });
        self.host_restart_required |= host_restart_required;
        self.inspector.reap();
        let Some(session) = &mut self.active else { return; };
        let mut events = session.poll();
        let done = session.reap();
        if done.is_some() { events.extend(session.poll()); }
        self.state = session.state();
        if let Some(Err(error)) = &done { self.state = DebugState::Failed(error.clone()); events.push(DebugEvent::Failed(error.clone())); }
        for event in &events {
            if let DebugEvent::Output { category, text } = event {
                if category == "stderr" { self.output.push_str("[stderr] "); }
                self.output.push_str(text);
            }
            let diagnostic = match event {
                DebugEvent::Initialized => Some("Adapter initialized".to_owned()),
                DebugEvent::Ready => Some("Debug session ready".to_owned()),
                DebugEvent::Stopped { thread_id, reason, all_threads_stopped } => Some(format!("Stopped: {reason} (thread {}, all threads stopped: {all_threads_stopped})", thread_id.map(|id| id.to_string()).unwrap_or_else(|| "unknown".into()))),
                DebugEvent::Continued { thread_id, all_threads_continued } => Some(format!("Continued: thread {}, all threads continued: {all_threads_continued}", thread_id.map(|id| id.to_string()).unwrap_or_else(|| "unknown".into()))),
                DebugEvent::Thread { thread_id, reason } => Some(format!("Thread {thread_id}: {reason}")),
                DebugEvent::Breakpoint { reason, breakpoint } => Some(format!("Breakpoint {reason}: {}", breakpoint)),
                DebugEvent::Exited { code } => Some(format!("Debuggee exited with code {code}")),
                DebugEvent::Terminated { exit_code, cancelled } => Some(format!("Session terminated (exit {}, cancelled: {cancelled})", exit_code.map(|code| code.to_string()).unwrap_or_else(|| "unreported".into()))),
                DebugEvent::Failed(error) => Some(format!("Debugger failed: {error}")),
                DebugEvent::AdapterEvent { event, body } => Some(format!("Adapter event {event}: {body}")),
                DebugEvent::Output { .. } => None,
            };
            if let Some(diagnostic) = diagnostic {
                self.event_log.push(diagnostic.chars().take(4096).collect());
            }
            match event {
                DebugEvent::Stopped { reason, .. } => self.stop_reason = Some(reason.clone()),
                DebugEvent::Continued { .. } | DebugEvent::Terminated { .. } => self.stop_reason = None,
                _ => {}
            }
        }
        if self.output.len() > 1_048_576 {
            let mut boundary = self.output.len() - 1_048_576;
            while !self.output.is_char_boundary(boundary) { boundary += 1; }
            self.output.drain(..boundary);
        }
        if self.event_log.len() > 200 { self.event_log.drain(..self.event_log.len() - 200); }
        self.summary = match &self.state {
            DebugState::Starting => "Starting Java debugger…".into(),
            DebugState::Running => "Java debuggee running".into(),
            DebugState::Stopped => "Java debuggee stopped".into(),
            DebugState::Disconnecting => "Disconnecting Java debugger…".into(),
            DebugState::Terminated { exit_code, cancelled } => format!("{} — exit {}", if *cancelled { "Debug stopped" } else { "Debug finished" }, exit_code.map(|code| code.to_string()).unwrap_or_else(|| "unreported".into())),
            DebugState::Failed(error) => error.to_string(),
        };
        let controller = session.controller();
        let ready = events.iter().any(|event| matches!(event, DebugEvent::Ready));
        let stopped_thread = events.iter().rev().find_map(|event| match event { DebugEvent::Stopped { thread_id, .. } => *thread_id, _ => None });
        for event in &events {
            if let DebugEvent::Breakpoint { reason, breakpoint }=event {self.breakpoints.adapter_event(reason,breakpoint);}
        }
        if ready {
            for source in self.breakpoints.sources.keys() {
                self.inspector.submit(controller.clone(), InspectRequest::Breakpoints { source: source.clone(), revision: self.breakpoints.source_revision(source), arguments: self.breakpoints.arguments(source) });
            }
        }
        self.inspector.poll(Some(controller.clone()), &mut self.breakpoints, stopped_thread);
        let frame = self.inspector.selected_frame;
        let watch_frame = frame.map(|frame| (self.inspector.epoch, frame));
        if matches!(self.state, DebugState::Stopped) && watch_frame != self.watched_frame {
            self.watched_frame = watch_frame;
            if let Some(frame) = frame {
                for expression in self.watches.clone() {
                    self.inspector.submit(controller.clone(), InspectRequest::Evaluate { frame, expression });
                }
            }
        }
        self.last_events = events;
        if done.is_some() {
            self.host_restart_required |= session.needs_host_restart();
            self.active = None;
            self.inspector.detach();
            self.watched_frame = None;
            self.breakpoints.clear_retained_events();
        }
    }
}
