//! Stdio JSON RPC client with worker-owned process and explicit shutdown/reaping.
use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, watch};
use url::Url;

use super::lsp_config::JdtlsConfig;
use crate::editor::Document;
use crate::project::ProjectModel;

#[derive(Clone, Debug)]
pub struct LspError(pub String);
impl fmt::Display for LspError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for LspError {}
impl From<std::io::Error> for LspError {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorCommand {
    Completion,
    SignatureHelp,
    Hover,
    Definition,
    TypeDefinition,
    Implementation,
    References,
    CodeActions,
    DocumentSymbols,
    Format,
    OrganizeImports,
    Rename,
}
impl EditorCommand {
    fn method(self) -> &'static str {
        match self {
            Self::Completion => "textDocument/completion",
            Self::SignatureHelp => "textDocument/signatureHelp",
            Self::Hover => "textDocument/hover",
            Self::Definition => "textDocument/definition",
            Self::TypeDefinition => "textDocument/typeDefinition",
            Self::Implementation => "textDocument/implementation",
            Self::References => "textDocument/references",
            Self::CodeActions => "textDocument/codeAction",
            Self::DocumentSymbols => "textDocument/documentSymbol",
            Self::Format => "textDocument/formatting",
            Self::OrganizeImports => "java/organizeImports",
            Self::Rename => "textDocument/rename",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentEditKind {
    Format,
    OrganizeImports,
}

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}
#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct LanguageDiagnostic {
    pub range: Range,
    pub severity: Option<u32>,
    pub message: String,
}
#[derive(Clone, Debug)]
pub struct Location {
    pub path: PathBuf,
    pub range: Range,
}
#[derive(Clone, Debug)]
pub struct WorkspaceFileEdits {
    pub path: PathBuf,
    pub edits: Vec<(Range, String)>,
    pub rename_to: Option<PathBuf>,
    pub version: Option<i64>,
}
#[derive(Clone, Debug)]
pub struct WorkspaceSymbol {
    pub name: String,
    pub kind: u64,
    pub container: Option<String>,
    pub location: Location,
}
#[derive(Clone, Debug)]
pub struct OutlineSymbol {
    pub name: String,
    pub kind: u64,
    pub location: Location,
    pub depth: usize,
}
#[derive(Clone, Debug)]
pub struct CodeActionItem {
    pub title: String,
    pub edits: Vec<(Range, String)>,
}
#[derive(Clone, Debug)]
pub struct CompletionItem {
    pub label: String,
    pub detail: String,
    pub insert_text: String,
    pub range: Option<Range>,
    pub additional_edits: Vec<(Range, String)>,
    pub snippet_tab_stops: Option<Vec<(u32, Vec<(usize, usize)>)>>,
    pub applicable: bool,
}

const MAX_COMPLETION_SNIPPET_CHARS: usize = 16 * 1024;

#[derive(Default)]
struct SnippetExpansion {
    text: String,
    tab_stops: std::collections::BTreeMap<u32, Vec<(usize, usize)>>,
}

struct SnippetParser {
    source: Vec<char>,
    cursor: usize,
    expansion: SnippetExpansion,
}

impl SnippetParser {
    fn parse(source: &str) -> Option<SnippetExpansion> {
        if source.chars().count() > MAX_COMPLETION_SNIPPET_CHARS {
            return None;
        }
        let mut parser = Self {
            source: source.chars().collect(),
            cursor: 0,
            expansion: SnippetExpansion::default(),
        };
        parser.sequence(false)?;
        if parser.cursor != parser.source.len() {
            return None;
        }
        if !parser.expansion.tab_stops.is_empty() && !parser.expansion.tab_stops.contains_key(&0) {
            let end = parser.expansion.text.chars().count();
            parser.expansion.tab_stops.insert(0, vec![(end, end)]);
        }
        Some(parser.expansion)
    }

    fn sequence(&mut self, nested: bool) -> Option<()> {
        while let Some(character) = self.source.get(self.cursor).copied() {
            match character {
                '}' if nested => {
                    self.cursor += 1;
                    return Some(());
                }
                '\\' => {
                    self.cursor += 1;
                    let escaped = self.source.get(self.cursor).copied()?;
                    if !matches!(escaped, '$' | '}' | '\\' | ',' | '|') {
                        return None;
                    }
                    self.expansion.text.push(escaped);
                    self.cursor += 1;
                }
                '$' => self.placeholder()?,
                _ => {
                    self.expansion.text.push(character);
                    self.cursor += 1;
                }
            }
        }
        (!nested).then_some(())
    }

    fn placeholder(&mut self) -> Option<()> {
        self.cursor += 1;
        if self.source.get(self.cursor) == Some(&'{') {
            self.cursor += 1;
            return self.braced_placeholder();
        }
        if self
            .source
            .get(self.cursor)
            .is_some_and(char::is_ascii_digit)
        {
            let index = self.number()?;
            let position = self.expansion.text.chars().count();
            let default = self.previous_stop_text(index);
            let start = position;
            if let Some(default) = default {
                self.expansion.text.push_str(&default);
            }
            self.record_stop(index, start, self.expansion.text.chars().count());
            return Some(());
        }
        if self
            .source
            .get(self.cursor)
            .is_some_and(|character| character.is_ascii_alphabetic() || *character == '_')
        {
            while self
                .source
                .get(self.cursor)
                .is_some_and(|character| character.is_ascii_alphanumeric() || *character == '_')
            {
                self.cursor += 1;
            }
            return Some(());
        }
        self.expansion.text.push('$');
        Some(())
    }

    fn braced_placeholder(&mut self) -> Option<()> {
        let start = self.cursor;
        while self
            .source
            .get(self.cursor)
            .is_some_and(|character| character.is_ascii_alphanumeric() || *character == '_')
        {
            self.cursor += 1;
        }
        if start == self.cursor {
            return None;
        }
        let token: String = self.source[start..self.cursor].iter().collect();
        let numeric = token.parse::<u32>().ok();
        let marker = *self.source.get(self.cursor)?;
        match marker {
            '}' => {
                self.cursor += 1;
                if let Some(index) = numeric {
                    let start = self.expansion.text.chars().count();
                    if let Some(default) = self.previous_stop_text(index) {
                        self.expansion.text.push_str(&default);
                    }
                    self.record_stop(index, start, self.expansion.text.chars().count());
                }
                Some(())
            }
            ':' => {
                if numeric.is_some_and(|index| self.expansion.tab_stops.contains_key(&index)) {
                    return None;
                }
                self.cursor += 1;
                let start = self.expansion.text.chars().count();
                self.sequence(true)?;
                let end = self.expansion.text.chars().count();
                if let Some(index) = numeric {
                    self.record_stop(index, start, end);
                }
                Some(())
            }
            '|' if numeric.is_some() => {
                if numeric.is_some_and(|index| self.expansion.tab_stops.contains_key(&index)) {
                    return None;
                }
                self.cursor += 1;
                let start = self.expansion.text.chars().count();
                self.choice_list()?;
                let end = self.expansion.text.chars().count();
                self.record_stop(numeric?, start, end);
                Some(())
            }
            _ => None,
        }
    }

    fn choice_list(&mut self) -> Option<()> {
        let mut first_choice = true;
        let mut found_end = false;
        while let Some(character) = self.source.get(self.cursor).copied() {
            if character == '|' && self.source.get(self.cursor + 1) == Some(&'}') {
                self.cursor += 2;
                found_end = true;
                break;
            }
            if character == '\\' {
                self.cursor += 1;
                let escaped = self.source.get(self.cursor).copied()?;
                if !matches!(escaped, '$' | '}' | '\\' | ',' | '|') {
                    return None;
                }
                if first_choice {
                    self.expansion.text.push(escaped);
                }
                self.cursor += 1;
            } else if character == ',' {
                first_choice = false;
                self.cursor += 1;
            } else {
                if matches!(character, '$' | '}') {
                    return None;
                }
                if first_choice {
                    self.expansion.text.push(character);
                }
                self.cursor += 1;
            }
        }
        found_end.then_some(())
    }

    fn number(&mut self) -> Option<u32> {
        let start = self.cursor;
        while self
            .source
            .get(self.cursor)
            .is_some_and(char::is_ascii_digit)
        {
            self.cursor += 1;
        }
        self.source[start..self.cursor]
            .iter()
            .collect::<String>()
            .parse()
            .ok()
    }

    fn record_stop(&mut self, index: u32, start: usize, end: usize) {
        self.expansion
            .tab_stops
            .entry(index)
            .or_default()
            .push((start, end));
    }

    fn previous_stop_text(&self, index: u32) -> Option<String> {
        let (start, end) = *self.expansion.tab_stops.get(&index)?.first()?;
        Some(
            self.expansion
                .text
                .chars()
                .skip(start)
                .take(end - start)
                .collect(),
        )
    }
}
#[derive(Clone, Debug)]
pub struct SignatureInformation {
    pub label: String,
    pub documentation: String,
    pub active_parameter: Option<String>,
}
#[derive(Clone, Debug)]
pub enum LanguageEvent {
    Diagnostics {
        path: PathBuf,
        version: Option<i64>,
        items: Vec<LanguageDiagnostic>,
    },
    Completion {
        path: PathBuf,
        version: i64,
        position: Position,
        items: Vec<CompletionItem>,
    },
    SignatureHelp {
        path: PathBuf,
        version: i64,
        signatures: Vec<SignatureInformation>,
        active_signature: usize,
    },
    Hover {
        path: PathBuf,
        version: i64,
        text: String,
    },
    Definition {
        path: PathBuf,
        version: i64,
        locations: Vec<Location>,
    },
    TypeDefinition {
        path: PathBuf,
        version: i64,
        locations: Vec<Location>,
    },
    Implementation {
        path: PathBuf,
        version: i64,
        locations: Vec<Location>,
    },
    References {
        path: PathBuf,
        version: i64,
        locations: Vec<Location>,
    },
    CodeActions {
        path: PathBuf,
        version: i64,
        position: Position,
        result: Result<Vec<CodeActionItem>, String>,
    },
    WorkspaceSymbols {
        query: String,
        result: Result<Vec<WorkspaceSymbol>, String>,
    },
    DocumentSymbols {
        path: PathBuf,
        version: i64,
        result: Result<Vec<OutlineSymbol>, String>,
    },
    DocumentEdits {
        path: PathBuf,
        version: i64,
        kind: DocumentEditKind,
        result: Result<Vec<(Range, String)>, String>,
    },
    RenameEdits {
        path: PathBuf,
        version: i64,
        position: Position,
        result: Result<Vec<WorkspaceFileEdits>, String>,
    },
    ServerState(String),
}

/// Session identity travels with the event through AppEvent queues. It is
/// checked when reducing the event, rather than only when polling the worker.
#[derive(Clone, Debug)]
pub struct LanguageUpdate {
    pub generation: u64,
    pub event: LanguageEvent,
}

#[derive(Clone)]
struct LanguageEventSender {
    generation: u64,
    sender: mpsc::Sender<LanguageUpdate>,
}
impl LanguageEventSender {
    async fn send(
        &self,
        event: LanguageEvent,
    ) -> Result<(), mpsc::error::SendError<LanguageUpdate>> {
        self.sender
            .send(LanguageUpdate {
                generation: self.generation,
                event,
            })
            .await
    }
    fn blocking_send(
        &self,
        event: LanguageEvent,
    ) -> Result<(), mpsc::error::SendError<LanguageUpdate>> {
        self.sender.blocking_send(LanguageUpdate {
            generation: self.generation,
            event,
        })
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeClasspaths {
    #[serde(default)]
    pub classpaths: Vec<PathBuf>,
    #[serde(default)]
    pub modulepaths: Vec<PathBuf>,
    #[serde(default, alias = "project_name")]
    pub project_name: Option<String>,
}
struct PendingClasspaths {
    sent: Instant,
    reply: oneshot::Sender<Result<RuntimeClasspaths, LspError>>,
    path: PathBuf,
    main_class: String,
    paths: Option<RuntimeClasspaths>,
}

fn main_project_name(
    result: &Value,
    main_class: &str,
    source: &Path,
) -> Result<Option<String>, LspError> {
    let items = result
        .as_array()
        .ok_or_else(|| LspError("Invalid Java main-class response".into()))?;
    let source = source
        .canonicalize()
        .unwrap_or_else(|_| source.to_path_buf());
    let mut names = std::collections::BTreeSet::new();
    for item in items {
        if item["mainClass"].as_str() != Some(main_class) {
            continue;
        }
        let Some(path) = item["filePath"].as_str().map(PathBuf::from) else {
            continue;
        };
        let path = path.canonicalize().unwrap_or(path);
        if path != source && !(source.is_dir() && path.starts_with(&source)) {
            continue;
        }
        if let Some(name) = item["projectName"].as_str().filter(|name| !name.is_empty()) {
            names.insert(name.to_owned());
        }
    }
    if names.len() > 1 {
        return Err(LspError(
            "Multiple Java projects contain the selected main class; choose a project module"
                .into(),
        ));
    }
    Ok(names.into_iter().next())
}

#[cfg(test)]
mod main_project_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires INTELLI_GEN_JAVA_HOME, INTELLI_GEN_JAVA_RESOURCES and INTELLI_GEN_DAP_PROJECT smoke fixture"]
    async fn live_worker_resolves_server_project_name() {
        let root = PathBuf::from(std::env::var_os("INTELLI_GEN_DAP_PROJECT").unwrap());
        let home = PathBuf::from(std::env::var_os("INTELLI_GEN_JAVA_HOME").unwrap());
        let resources = PathBuf::from(std::env::var_os("INTELLI_GEN_JAVA_RESOURCES").unwrap());
        let jdk = crate::java::jdk::JdkCatalog::add(&home).unwrap();
        let executable = resources.parent().unwrap().join("MacOS/intelli-gen");
        let resolved = crate::java::lsp_config::resolve_jdtls(
            &crate::settings::UserSettings::default(),
            &executable,
        )
        .unwrap();
        let bundle = crate::java::lsp_config::resolve_debug_bundle(
            &crate::settings::UserSettings::default(),
            &executable,
        )
        .unwrap();
        let config = JdtlsConfig {
            executable: resolved.display_path,
            launcher_arguments: resolved.arguments,
            lombok_agent: None,
            lombok_declared: false,
            shared_configuration: resolved.shared_configuration,
            package_version: resolved.package_version,
            runtime_jdk_id: jdk.id.clone(),
            project_jdk_id: Some(jdk.id.clone()),
            gradle_offline: true,
            workspace_data: root.parent().unwrap().join("rust-worker-workspace"),
            jdks: vec![jdk],
            debug_bundle: Some(bundle),
            debug_bundle_error: None,
        };
        let project = ProjectModel {
            root: root.clone(),
            name: "directory-fallback".into(),
            kind: crate::project::ProjectKind::Unknown,
            modules: Vec::new(),
            wrapper: None,
        };
        let mut session = JavaLanguageService::start(&project, &config).unwrap();
        tokio::time::timeout(Duration::from_secs(90), async {
            loop {
                let update = session.events.recv().await.expect("language worker event");
                if let LanguageEvent::ServerState(message) = update.event {
                    assert!(
                        !message.starts_with("Java intelligence unavailable:"),
                        "{message}"
                    );
                    if message == "Java intelligence ready" {
                        break;
                    }
                }
            }
        })
        .await
        .expect("language worker ready");
        let resolver = ClasspathResolver {
            commands: session.commands.clone(),
            root,
        };
        let paths = resolver.resolve(&project, "Main", None).await.unwrap();
        assert_eq!(paths.project_name.as_deref(), Some("smoke"));
        assert!(paths.classpaths.iter().any(|path| path.ends_with("bin")));
        let resolver = DebugAdapterResolver {
            commands: session.commands.clone(),
            root: project.root.clone(),
            bundle: config.debug_bundle.clone(),
        };
        let service = crate::debugger::DebugService::new(
            resolver,
            config.debug_bundle.clone().unwrap(),
        )
        .with_initial_requests(vec![(
            "setBreakpoints".into(),
            json!({"source":{"path":project.root.join("Main.java")},"breakpoints":[{"line":4}]}),
        )]);
        let mut run = crate::java::run_config::RunConfiguration::new(&project.root);
        run.main_class = "Main".into();
        let mut debug = service.start(&run, &project, &config.jdks[0]).unwrap();
        async fn stopped(debug: &mut crate::debugger::session::DebugSession) -> i64 {
            tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    for event in debug.poll() {
                        match event {
                            crate::debugger::session::DebugEvent::Stopped { thread_id, .. } => {
                                return thread_id.unwrap();
                            }
                            crate::debugger::session::DebugEvent::Failed(error) => {
                                panic!("debug worker failed: {error}")
                            }
                            _ => {}
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("debug worker stopped at breakpoint")
        }
        let thread = stopped(&mut debug).await;
        assert!(matches!(
            debug.state(),
            crate::debugger::DebugState::Stopped
        ));
        let client = debug.client();
        let stack = client
            .request("stackTrace", json!({"threadId":thread}))
            .await
            .unwrap();
        let frame = &stack["stackFrames"][0];
        assert_eq!(frame["line"], 4);
        let value = client
            .request(
                "evaluate",
                json!({"frameId":frame["id"],"expression":"value + 2","context":"watch"}),
            )
            .await
            .unwrap();
        assert_eq!(value["result"], "42");
        client
            .request("next", json!({"threadId":thread}))
            .await
            .unwrap();
        let thread = stopped(&mut debug).await;
        let stack = client
            .request("stackTrace", json!({"threadId":thread}))
            .await
            .unwrap();
        assert_eq!(stack["stackFrames"][0]["line"], 5);
        client
            .request("continue", json!({"threadId":thread}))
            .await
            .unwrap();
        let mut output_seen = false;
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                for event in debug.poll() {
                    match event {
                        crate::debugger::session::DebugEvent::Failed(error) => {
                            panic!("debug worker failed: {error}")
                        }
                        crate::debugger::session::DebugEvent::Output { text, .. } => {
                            output_seen |= text.lines().any(|line| line.trim() == "42")
                        }
                        _ => {}
                    }
                }
                if let Some(result) = debug.reap() {
                    result.unwrap();
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("debug worker terminated and joined");
        match debug.state() {
            crate::debugger::DebugState::Terminated {
                exit_code,
                cancelled: false,
            } => {
                if let Some(code) = exit_code {
                    assert_eq!(code, 0);
                }
            }
            state => panic!("unexpected final state: {state:?}"),
        }
        assert!(output_seen, "debuggee must print its final value");
        assert!(!debug.needs_host_restart());
        let source = project.root.join("Main.java");
        let mut points = crate::debugger::breakpoints::Breakpoints::default();
        points.set_log_message(source.clone(), 4, "smoke={value}".into());
        points.update(
            &source,
            4,
            Some("value == 40".into()),
            Some("smoke={value}".into()),
            true,
        );
        points.toggle(source, 5);
        let resolver = DebugAdapterResolver {
            commands: session.commands.clone(),
            root: project.root.clone(),
            bundle: config.debug_bundle.clone(),
        };
        let service =
            crate::debugger::DebugService::new(resolver, config.debug_bundle.clone().unwrap())
                .with_initial_requests(points.initial_requests());
        let mut second = service.start(&run, &project, &config.jdks[0]).unwrap();
        let mut log = String::new();
        let thread = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let mut stopped_thread = None;
                for event in second.poll() {
                    match event {
                        crate::debugger::session::DebugEvent::Output { text, .. } => {
                            log.push_str(&text)
                        }
                        crate::debugger::session::DebugEvent::Stopped { thread_id, .. } => {
                            stopped_thread = thread_id
                        }
                        crate::debugger::session::DebugEvent::Failed(error) => {
                            panic!("second debug session failed: {error}")
                        }
                        _ => {}
                    }
                }
                if let Some(thread) = stopped_thread {
                    break thread;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("second session reached the suspending breakpoint");
        assert!(
            log.contains("smoke=40"),
            "conditional logpoint output: {log}"
        );
        let stack = second
            .client()
            .request("stackTrace", json!({"threadId":thread}))
            .await
            .unwrap();
        assert_eq!(
            stack["stackFrames"][0]["line"], 5,
            "logpoint must not suspend at line 4"
        );
        second.cancel();
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                second.poll();
                if let Some(result) = second.reap() {
                    result.unwrap();
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("cancelled debug session reaped");
        assert!(matches!(
            second.state(),
            crate::debugger::DebugState::Terminated {
                cancelled: true,
                ..
            }
        ));
        assert!(!second.needs_host_restart());
        session.shutdown();
        session.worker.take().unwrap().join().unwrap();
    }

    #[test]
    fn server_project_name_is_selected_by_main_class_and_source() {
        let result = json!([
            {"mainClass":"Main","projectName":"wrong-module","filePath":"/workspace/other/Main.java"},
            {"mainClass":"Main","projectName":"logical-gradle-name","filePath":"/workspace/custom/service/Main.java"}
        ]);
        assert_eq!(
            main_project_name(
                &result,
                "Main",
                Path::new("/workspace/custom/service/Main.java")
            )
            .unwrap()
            .as_deref(),
            Some("logical-gradle-name")
        );
        assert!(
            main_project_name(
                &result,
                "Missing",
                Path::new("/workspace/custom/service/Main.java")
            )
            .unwrap()
            .is_none()
        );
        assert!(
            main_project_name(&json!(null), "Main", Path::new("/workspace/Main.java")).is_err()
        );
    }
}

/// This sender belongs to one language session. Restarting drops its receiver;
/// a response cannot be delivered by a replacement session.
#[derive(Clone)]
pub struct ClasspathResolver {
    commands: mpsc::Sender<LanguageCommand>,
    root: PathBuf,
}
impl ClasspathResolver {
    pub async fn resolve(
        &self,
        project: &ProjectModel,
        main_class: &str,
        project_module: Option<&Path>,
    ) -> Result<RuntimeClasspaths, LspError> {
        if project.root != self.root {
            return Err(LspError(
                "Classpath resolver belongs to another project".into(),
            ));
        }
        let relative = format!(
            "{}.java",
            main_class
                .replace('.', "/")
                .split('$')
                .next()
                .unwrap_or_default()
        );
        let module_name = |module_root: &Path, fallback: &str| match project.kind {
            crate::project::ProjectKind::Maven => crate::project::maven::artifact_id(module_root)
                .ok()
                .flatten()
                .unwrap_or_else(|| {
                    module_root
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or(fallback)
                        .to_owned()
                }),
            crate::project::ProjectKind::Gradle => module_root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(fallback)
                .to_owned(),
            crate::project::ProjectKind::Unknown => fallback.to_owned(),
        };
        let mut projects = Vec::new();
        if let Some(module_path) = project_module {
            if module_path.is_absolute()
                || module_path.components().any(|part| {
                    matches!(
                        part,
                        std::path::Component::ParentDir
                            | std::path::Component::RootDir
                            | std::path::Component::Prefix(_)
                    )
                })
            {
                return Err(LspError(
                    "Project module must be a path relative to the build root".into(),
                ));
            }
            let selected_root = project.root.join(module_path);
            if selected_root == project.root {
                projects.push((project.root.as_path(), project.name.clone()));
            } else if let Some(module) = project
                .modules
                .iter()
                .find(|module| module.root == selected_root)
            {
                projects.push((
                    module.root.as_path(),
                    module_name(&module.root, &module.name),
                ));
            } else {
                return Err(LspError(format!(
                    "Selected project module no longer exists: {}",
                    module_path.display()
                )));
            }
        } else {
            projects.push((project.root.as_path(), project.name.clone()));
            for module in &project.modules {
                let fallback_name = || {
                    module
                        .root
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or(&module.name)
                        .to_owned()
                };
                projects.push((
                    module.root.as_path(),
                    module_name(&module.root, &fallback_name()),
                ));
            }
        }
        let module = projects.into_iter().find_map(|(root, name)| {
            ["src/main/java", "src/test/java"]
                .into_iter()
                .map(|source_root| root.join(source_root).join(&relative))
                .find(|source| source.is_file())
                .map(|source| (source, name))
        });
        let (source, project_name) = match (module, project_module) {
            (Some(module), _) => module,
            (None, Some(selected)) => {
                return Err(LspError(format!(
                    "Main class {main_class} was not found in project module {}",
                    selected.display()
                )));
            }
            (None, None) => (project.root.clone(), project.name.clone()),
        };
        let (reply, receiver) = oneshot::channel();
        self.commands
            .try_send(LanguageCommand::Classpaths {
                path: source,
                main_class: main_class.to_owned(),
                reply,
            })
            .map_err(|error| LspError(format!("Java language service unavailable: {error}")))?;
        tokio::time::timeout(Duration::from_secs(90), receiver)
            .await
            .map_err(|_| LspError("Runtime classpath resolution timed out".into()))?
            .map_err(|_| {
                LspError("Language session ended while resolving runtime classpath".into())
            })?
            .map(|mut paths| {
                if paths.project_name.as_deref().is_none_or(str::is_empty) {
                    paths.project_name = Some(project_name);
                }
                paths
            })
    }
}

/// A capability tied to the current JDT LS worker, build root and configured
/// bundle. A replacement language session cannot answer its requests.
#[derive(Clone)]
pub struct DebugAdapterResolver {
    commands: mpsc::Sender<LanguageCommand>,
    root: PathBuf,
    bundle: Option<PathBuf>,
}
impl DebugAdapterResolver {
    pub fn validate(&self, root: &Path, bundle: &Path) -> Result<(), LspError> {
        if root != self.root {
            return Err(LspError(
                "Debug adapter resolver belongs to another project".into(),
            ));
        }
        let loaded = self
            .bundle
            .as_deref()
            .and_then(|path| crate::debugger::validate_bundle(path).ok());
        if loaded.as_deref() != Some(bundle) {
            return Err(LspError("The current JDT LS session was not configured with this Java Debug Server bundle; apply / restart Java intelligence".into()));
        }
        if self.commands.is_closed() {
            return Err(LspError("The Java language session has stopped".into()));
        }
        Ok(())
    }
    pub async fn resolve_classpaths(
        &self,
        project: &ProjectModel,
        main_class: &str,
        project_module: Option<&Path>,
    ) -> Result<RuntimeClasspaths, LspError> {
        ClasspathResolver {
            commands: self.commands.clone(),
            root: self.root.clone(),
        }
        .resolve(project, main_class, project_module)
        .await
    }
    pub async fn start_debug_session(&self) -> Result<u16, LspError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .try_send(LanguageCommand::StartDebugSession { reply })
            .map_err(|error| LspError(format!("Java debug server command unavailable: {error}")))?;
        tokio::time::timeout(Duration::from_secs(90), response)
            .await
            .map_err(|_| LspError("JDT LS startDebugSession timed out".into()))?
            .map_err(|_| LspError("Language session ended during startDebugSession".into()))?
    }
}

enum LanguageCommand {
    StartDebugSession {
        reply: oneshot::Sender<Result<u16, LspError>>,
    },
    RefreshMaven {
        root_pom: PathBuf,
    },
    Classpaths {
        path: PathBuf,
        main_class: String,
        reply: oneshot::Sender<Result<RuntimeClasspaths, LspError>>,
    },
    Open {
        path: PathBuf,
        text: String,
        version: i64,
    },
    Change {
        path: PathBuf,
        text: String,
        version: i64,
    },
    Save(PathBuf),
    Close(PathBuf),
    Request {
        command: EditorCommand,
        path: PathBuf,
        project_root: PathBuf,
        position: Position,
        document_end: Position,
        range: Range,
        diagnostics: Vec<LanguageDiagnostic>,
        version: i64,
        rename_to: Option<String>,
        tab_size: usize,
        insert_spaces: bool,
    },
    SearchSymbols {
        query: String,
    },
}

pub struct LanguageSession {
    commands: mpsc::Sender<LanguageCommand>,
    events: mpsc::Receiver<LanguageUpdate>,
    generation: u64,
    debug_bundle: Option<PathBuf>,
    shutdown: watch::Sender<bool>,
    worker: Option<JoinHandle<()>>,
}
impl LanguageSession {
    pub fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }
    fn try_command(&self, command: LanguageCommand) -> Result<(), LspError> {
        self.commands
            .try_send(command)
            .map_err(|error| LspError(format!("Java service is busy or stopped: {error}")))
    }
    fn reap(&mut self) -> Option<Result<(), String>> {
        if !self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            return None;
        }
        Some(
            self.worker
                .take()
                .unwrap()
                .join()
                .map_err(|_| "Java language worker panicked".into()),
        )
    }
}
impl Drop for LanguageSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub struct JavaLanguageService;
impl JavaLanguageService {
    /// Creates a worker immediately. Filesystem validation and process launch are
    /// asynchronous and reported as ServerState events, so editing remains usable.
    pub fn start(
        project: &ProjectModel,
        config: &JdtlsConfig,
    ) -> Result<LanguageSession, LspError> {
        Self::start_for_generation(project, config, 0)
    }

    fn start_for_generation(
        project: &ProjectModel,
        config: &JdtlsConfig,
        generation: u64,
    ) -> Result<LanguageSession, LspError> {
        let project = project.clone();
        let config = config.clone();
        let debug_bundle = config.debug_bundle.clone();
        let (commands, requests) = mpsc::channel(64);
        let (sender, events) = mpsc::channel(128);
        let sender = LanguageEventSender { generation, sender };
        let (shutdown, cancellation) = watch::channel(false);
        let worker = std::thread::Builder::new()
            .name("java-language".into())
            .spawn(move || {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime.block_on(async {
                        if let Err(error) =
                            run(project, config, requests, sender.clone(), cancellation).await
                        {
                            let _ = sender
                                .send(LanguageEvent::ServerState(format!(
                                    "Java intelligence unavailable: {error}"
                                )))
                                .await;
                        }
                    }),
                    Err(error) => {
                        let _ = sender.blocking_send(LanguageEvent::ServerState(format!(
                            "Cannot create language runtime: {error}"
                        )));
                    }
                }
            })
            .map_err(|error| LspError(format!("Cannot start language worker: {error}")))?;
        Ok(LanguageSession {
            commands,
            events,
            generation,
            debug_bundle,
            shutdown,
            worker: Some(worker),
        })
    }
}

async fn send(writer: &mut (impl AsyncWrite + Unpin), message: Value) -> Result<(), LspError> {
    let bytes = serde_json::to_vec(&message).map_err(|error| LspError(error.to_string()))?;
    tokio::time::timeout(Duration::from_secs(3), async {
        writer
            .write_all(format!("Content-Length: {}\r\n\r\n", bytes.len()).as_bytes())
            .await?;
        writer.write_all(&bytes).await?;
        writer.flush().await
    })
    .await
    .map_err(|_| LspError("JDT LS write timed out".into()))??;
    Ok(())
}
async fn notify(
    writer: &mut (impl AsyncWrite + Unpin),
    method: &str,
    params: Value,
) -> Result<(), LspError> {
    send(
        writer,
        json!({"jsonrpc": "2.0", "method": method, "params": params}),
    )
    .await
}
fn uri(path: &Path) -> Result<String, LspError> {
    Url::from_file_path(path)
        .map(|uri| uri.to_string())
        .map_err(|_| LspError(format!("Cannot encode file URI: {}", path.display())))
}
fn document_end_position(text: &str) -> Position {
    let line = text.bytes().filter(|byte| *byte == b'\n').count();
    let last_line = text.rsplit('\n').next().unwrap_or_default();
    let last_line = last_line.strip_suffix('\r').unwrap_or(last_line);
    Position {
        line: u32::try_from(line).unwrap_or(u32::MAX),
        character: u32::try_from(last_line.encode_utf16().count()).unwrap_or(u32::MAX),
    }
}
fn document_line_range(text: &str, line: u32) -> Range {
    let contents = text.lines().nth(line as usize).unwrap_or_default();
    Range {
        start: Position { line, character: 0 },
        end: Position {
            line,
            character: u32::try_from(contents.encode_utf16().count()).unwrap_or(u32::MAX),
        },
    }
}
fn file_path(value: &Value) -> Option<PathBuf> {
    Url::parse(value.as_str()?).ok()?.to_file_path().ok()
}

async fn read_messages(
    reader: impl AsyncRead + Unpin,
    messages: mpsc::Sender<Result<Value, LspError>>,
) {
    let mut reader = BufReader::new(reader);
    loop {
        let next = async {
            let mut length = None;
            let mut header_size = 0;
            loop {
                // Byte-bounded headers prevent an accidental launcher banner from
                // growing memory without limit. This task alone owns the reader.
                let mut line = Vec::new();
                loop {
                    let byte = reader.read_u8().await.map_err(|error| {
                        if error.kind() == std::io::ErrorKind::UnexpectedEof {
                            LspError("JDT LS closed stdout before completing an LSP message".into())
                        } else {
                            LspError(error.to_string())
                        }
                    })?;
                    header_size += 1;
                    if header_size > 8192 {
                        return Err(LspError("LSP headers exceed 8 KiB".into()));
                    }
                    line.push(byte);
                    if byte == b'\n' {
                        break;
                    }
                }
                if line == b"\r\n" || line == b"\n" {
                    break;
                }
                let line =
                    String::from_utf8(line).map_err(|_| LspError("Invalid LSP header".into()))?;
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("Content-Length") {
                        length = Some(
                            value
                                .trim()
                                .parse::<usize>()
                                .map_err(|_| LspError("Invalid LSP Content-Length".into()))?,
                        );
                    }
                }
            }
            let length = length
                .filter(|size| *size <= 16 * 1024 * 1024)
                .ok_or_else(|| LspError("Missing or oversized LSP Content-Length".into()))?;
            let mut body = vec![0; length];
            reader.read_exact(&mut body).await.map_err(|error| {
                if error.kind() == std::io::ErrorKind::UnexpectedEof {
                    LspError("JDT LS closed stdout before completing an LSP message".into())
                } else {
                    LspError(error.to_string())
                }
            })?;
            serde_json::from_slice(&body).map_err(|error| {
                if error.is_eof() {
                    LspError(format!(
                        "JDT LS sent an incomplete JSON message ({} bytes; line {}, column {})",
                        body.len(),
                        error.line(),
                        error.column()
                    ))
                } else {
                    LspError(format!("Invalid LSP JSON: {error}"))
                }
            })
        }
        .await;
        let failed = next.is_err();
        if messages.send(next).await.is_err() || failed {
            return;
        }
    }
}

async fn run(
    project: ProjectModel,
    config: JdtlsConfig,
    mut commands: mpsc::Receiver<LanguageCommand>,
    events: LanguageEventSender,
    mut cancellation: watch::Receiver<bool>,
) -> Result<(), LspError> {
    if *cancellation.borrow() {
        return Ok(());
    }
    let runtime = config.runtime()?;
    let settings = config.java_settings()?;
    if !config.executable.is_absolute() || !config.executable.is_file() {
        return Err(LspError(
            "Select an existing absolute JDT LS launcher path in Settings".into(),
        ));
    }
    std::fs::create_dir_all(&config.workspace_data)?;
    let mut launcher_arguments = config.launcher_arguments.clone();
    // Lombok patches JDT inside the language-server JVM, so attach the project's
    // existing jar before Equinox starts. Do not affect custom launchers whose
    // argument contract is user-defined.
    if config.shared_configuration.is_some()
        && let Some(agent) = &config.lombok_agent
    {
        let mut argument = OsString::from("-javaagent:");
        argument.push(agent.as_os_str());
        let position = launcher_arguments
            .iter()
            .position(|value| value == "-jar")
            .unwrap_or(launcher_arguments.len());
        launcher_arguments.insert(position, argument);
    }
    if let (Some(shared), Some(version)) = (&config.shared_configuration, &config.package_version) {
        let configuration = config.workspace_data.join("configuration").join(version);
        if !configuration.exists() {
            copy_configuration_tree(shared, &configuration)?;
        }
        ensure_configuration_plugins_link(shared, &configuration)?;
        let position = launcher_arguments
            .iter()
            .position(|argument| argument == "-configuration")
            .ok_or_else(|| {
                LspError("Bundled JDT LS launcher is missing its configuration argument".into())
            })?;
        let target = launcher_arguments
            .get_mut(position + 1)
            .ok_or_else(|| LspError("Bundled JDT LS configuration path is missing".into()))?;
        *target = configuration.into_os_string();
    }
    let program = if config.launcher_arguments.is_empty() {
        config.executable.clone()
    } else {
        runtime.home.join("bin/java")
    };
    let mut child_environment = runtime
        .child_environment()
        .map_err(|error| LspError(error.to_string()))?;
    if config.shared_configuration.is_none()
        && let Some(agent) = &config.lombok_agent
    {
        let mut options = std::env::var_os("JAVA_TOOL_OPTIONS").unwrap_or_default();
        let agent_text = agent.to_string_lossy();
        if !options.to_string_lossy().contains(agent_text.as_ref()) {
            if !options.is_empty() {
                options.push(" ");
            }
            options.push("-javaagent:");
            let quote = if agent_text.chars().any(char::is_whitespace)
                || agent_text.contains('"')
                || agent_text.contains('\'')
            {
                if !agent_text.contains('"') {
                    Some('"')
                } else if !agent_text.contains('\'') {
                    Some('\'')
                } else {
                    return Err(LspError(
                        "Cannot quote the Lombok JAR path for the custom JDT LS launcher".into(),
                    ));
                }
            } else {
                None
            };
            if let Some(quote) = quote {
                options.push(quote.to_string());
            }
            options.push(agent.as_os_str());
            if let Some(quote) = quote {
                options.push(quote.to_string());
            }
            child_environment.push(("JAVA_TOOL_OPTIONS".into(), options));
        }
    }
    let mut command = tokio::process::Command::new(program);
    command
        .args(&launcher_arguments)
        .arg("-data")
        .arg(&config.workspace_data)
        .current_dir(&project.root)
        .envs(child_environment)
        // JDT LS uses stdio unless one of these inherited socket settings exists.
        .env_remove("CLIENT_PORT")
        .env_remove("CLIENT_HOST")
        .env_remove("SERVER_PORT")
        .env_remove("SERVER_HOST")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(|error| {
        LspError(format!(
            "Cannot launch {}: {error}",
            config.executable.display()
        ))
    })?;
    let pid = child.id();
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| LspError("Missing language server stdin".into()))?;
    let output = child
        .stdout
        .take()
        .ok_or_else(|| LspError("Missing language server stdout".into()))?;
    let errors = child
        .stderr
        .take()
        .ok_or_else(|| LspError("Missing language server stderr".into()))?;
    let (sender, mut messages) = mpsc::channel(64);
    let reader = tokio::spawn(read_messages(output, sender));
    let stderr = tokio::spawn(async move {
        // Drain stderr in bounded chunks. Preserve only the latest tail for failures.
        let mut errors = errors;
        let mut chunk = [0u8; 2048];
        let mut tail = Vec::new();
        while let Ok(count) = errors.read(&mut chunk).await {
            if count == 0 {
                break;
            }
            tail.extend_from_slice(&chunk[..count]);
            if tail.len() > 8192 {
                tail.drain(..tail.len() - 8192);
            }
        }
        String::from_utf8_lossy(&tail).into_owned()
    });
    let _ = events
        .send(LanguageEvent::ServerState(format!(
            "Starting JDT LS with {} (project runtime configured separately)",
            runtime.label()
        )))
        .await;
    // Invalid optional debug configuration must not disable ordinary Java editing.
    let bundles: Vec<_> = config
        .debug_bundle
        .as_deref()
        .and_then(|path| crate::debugger::validate_bundle(path).ok())
        .into_iter()
        .collect();
    if let Some(error) = &config.debug_bundle_error {
        let _ = events
            .send(LanguageEvent::ServerState(format!(
                "Java Debug Server unavailable: {error}"
            )))
            .await;
    } else if config.debug_bundle.is_some() && bundles.is_empty() {
        let _ = events
            .send(LanguageEvent::ServerState(
                "Java Debug Server bundle is invalid; configure its plugin JAR in Settings".into(),
            ))
            .await;
    }
    let result = protocol(
        &project,
        &bundles,
        settings,
        &mut input,
        &mut messages,
        &mut commands,
        &events,
        &mut cancellation,
    )
    .await;
    // After protocol shutdown/exit, allow a short asynchronous grace period and
    // always reap. A misbehaving launcher or pipe descendant cannot hold the UI.
    let exit = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    let exit_status = match exit {
        Ok(Ok(status)) => Some(status),
        _ => {
            kill_group(pid);
            let _ = child.start_kill();
            child.wait().await.ok()
        }
    };
    kill_group(pid);
    reader.abort();
    let _ = reader.await;
    let mut stderr = stderr;
    let tail = match tokio::time::timeout(Duration::from_millis(200), &mut stderr).await {
        Ok(Ok(tail)) => tail,
        _ => {
            stderr.abort();
            let _ = stderr.await;
            String::new()
        }
    };
    if let Err(error) = result {
        let status = exit_status
            .map(|status| format!("JDT LS process exited with {status}"))
            .unwrap_or_else(|| "JDT LS process exit status is unavailable".into());
        let stderr = tail.trim();
        return Err(LspError(if stderr.is_empty() {
            format!("{}\n{}", error, status)
        } else {
            format!("{}\n{}\nJDT LS stderr:\n{}", error, status, stderr)
        }));
    }
    let _ = events
        .send(LanguageEvent::ServerState(
            "Java language service stopped".into(),
        ))
        .await;
    Ok(())
}

fn copy_configuration_tree(source: &Path, destination: &Path) -> Result<(), LspError> {
    if destination.exists() {
        return Ok(());
    }
    let parent = destination
        .parent()
        .ok_or_else(|| LspError("JDT LS configuration path has no parent".into()))?;
    std::fs::create_dir_all(parent)?;
    let staging = parent.join(format!(".configuration-stage-{}", std::process::id()));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir(&staging)?;
    if let Err(error) = copy_configuration_contents(source, &staging) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }
    match std::fs::rename(&staging, destination) {
        Ok(()) => Ok(()),
        Err(_) if destination.is_dir() => {
            std::fs::remove_dir_all(&staging)?;
            Ok(())
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            Err(error.into())
        }
    }
}

fn copy_configuration_contents(source: &Path, destination: &Path) -> Result<(), LspError> {
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            std::fs::create_dir_all(&destination_path)?;
            copy_configuration_contents(&source_path, &destination_path)?;
        } else {
            if let Some(parent) = destination_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&source_path, &destination_path)?;
        }
    }
    Ok(())
}

fn ensure_configuration_plugins_link(
    shared_configuration: &Path,
    writable_configuration: &Path,
) -> Result<(), LspError> {
    let source = shared_configuration
        .parent()
        .ok_or_else(|| LspError("JDT LS shared configuration has no parent".into()))?
        .join("plugins");
    if !source.is_dir() {
        return Err(LspError(format!(
            "Bundled JDT LS plugins directory is missing: {}",
            source.display()
        )));
    }
    let link = writable_configuration.join("plugins");
    if let Ok(metadata) = std::fs::symlink_metadata(&link) {
        if metadata.file_type().is_symlink()
            && std::fs::canonicalize(&link).ok() == std::fs::canonicalize(&source).ok()
        {
            return Ok(());
        }
        if !metadata.file_type().is_symlink() {
            return Err(LspError(format!(
                "JDT LS configuration plugins path is not a symbolic link: {}",
                link.display()
            )));
        }
        std::fs::remove_file(&link)?;
    }
    create_directory_symlink(&source, &link).map_err(|error| {
        LspError(format!(
            "Cannot link writable JDT LS configuration to bundled plugins: {error}"
        ))
    })
}

#[cfg(unix)]
fn create_directory_symlink(source: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, link)
}

#[cfg(windows)]
fn create_directory_symlink(source: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(source, link)
}

#[cfg(not(any(unix, windows)))]
fn create_directory_symlink(_source: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "directory symbolic links are unsupported on this platform",
    ))
}

fn kill_group(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid.filter(|pid| *pid > 1) {
        // SAFETY: this PID identifies the process group created by our launcher.
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = pid;
}

struct Pending {
    command: EditorCommand,
    path: PathBuf,
    project_root: PathBuf,
    version: i64,
    position: Position,
    rename_to: Option<String>,
    sent: Instant,
}

async fn protocol(
    project: &ProjectModel,
    bundles: &[PathBuf],
    mut settings: Value,
    input: &mut (impl AsyncWrite + Unpin),
    messages: &mut mpsc::Receiver<Result<Value, LspError>>,
    commands: &mut mpsc::Receiver<LanguageCommand>,
    events: &LanguageEventSender,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<(), LspError> {
    let root_uri = uri(&project.root)?;
    send(input, json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "processId": std::process::id(), "clientInfo": {"name": "Intelli-gen", "version": env!("CARGO_PKG_VERSION")},
        "rootUri": root_uri, "workspaceFolders": [{"uri": root_uri, "name": project.root.file_name().unwrap_or_default().to_string_lossy()}],
        "capabilities": {"general": {"positionEncodings": ["utf-16"]},
            "workspace": {"configuration": true, "workspaceFolders": true, "applyEdit": false,
                "workspaceEdit": {"documentChanges": true, "resourceOperations": ["rename"]},
                "didChangeWatchedFiles": {"dynamicRegistration": true},
                "symbol": {"symbolKind": {"valueSet": (1..=26).collect::<Vec<_>>()}}},
                "textDocument": {"synchronization": {"didSave": true}, "publishDiagnostics": {"versionSupport": true},
                "documentSymbol": {"hierarchicalDocumentSymbolSupport": true},
                "codeAction": {"codeActionLiteralSupport": {"codeActionKind": {"valueSet": ["quickfix", "refactor", "source"]}}},
                "completion": {"completionItem": {"snippetSupport": true, "resolveSupport": {"properties": []}}},
                "signatureHelp": {"signatureInformation": {"documentationFormat": ["plaintext"]}},
                "hover": {"contentFormat": ["plaintext"]}, "definition": {"linkSupport": true},
                "implementation": {"linkSupport": true},
                "typeDefinition": {"linkSupport": true}}},
        "initializationOptions": {"settings": settings.clone(), "workspaceFolders": [root_uri], "bundles": bundles,
            "extendedClientCapabilities": {"classFileContentsSupport": false}}
    }})).await?;
    let ready = {
        let initialized = tokio::time::timeout(Duration::from_secs(90), async {
            loop {
                let message = messages
                    .recv()
                    .await
                    .ok_or_else(|| LspError("JDT LS closed during initialize".into()))??;
                if message.get("id").and_then(Value::as_u64) == Some(1)
                    && message.get("method").is_none()
                {
                    if let Some(error) = message.get("error") {
                        return Err(LspError(format!("JDT LS initialize failed: {error}")));
                    }
                    if message["result"]["capabilities"]["positionEncoding"]
                        .as_str()
                        .is_some_and(|encoding| encoding != "utf-16")
                    {
                        return Err(LspError(
                            "JDT LS selected an unsupported position encoding".into(),
                        ));
                    }
                    return Ok(());
                }
                server_message(input, &message, &settings, &root_uri, events).await?;
            }
        });
        tokio::pin!(initialized);
        tokio::select! {
            _ = cancellation.changed() => false,
            result = &mut initialized => { result.map_err(|_| LspError("JDT LS initialize timed out after 90 seconds".into()))??; true }
        }
    };
    if !ready {
        return Ok(());
    }
    notify(input, "initialized", json!({})).await?;
    notify(
        input,
        "workspace/didChangeConfiguration",
        json!({"settings": settings.clone()}),
    )
    .await?;
    let _ = events
        .send(LanguageEvent::ServerState("Java intelligence ready".into()))
        .await;
    let mut next_id = 2u64;
    let mut pending: HashMap<u64, Pending> = HashMap::new();
    let mut pending_symbols: HashMap<u64, (String, Instant)> = HashMap::new();
    let mut debug_ports: HashMap<u64, (Instant, oneshot::Sender<Result<u16, LspError>>)> =
        HashMap::new();
    let mut classpaths: HashMap<u64, PendingClasspaths> = HashMap::new();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(1));
    loop {
        if *cancellation.borrow() {
            break;
        }
        tokio::select! {
            _ = cancellation.changed() => break,
            command = commands.recv() => {
                let Some(command) = command else { break; };
                match command {
                    LanguageCommand::StartDebugSession { reply } => {
                        if bundles.is_empty() { let _ = reply.send(Err(LspError("Configure a valid Java Debug Server bundle and restart Java intelligence".into()))); continue; }
                        if debug_ports.len() >= 2 { let _ = reply.send(Err(LspError("A Java debug server start request is already pending".into()))); continue; }
                        let id = next_id; next_id += 1;
                        debug_ports.insert(id, (Instant::now(), reply));
                        send(input, json!({"jsonrpc": "2.0", "id": id, "method": "workspace/executeCommand", "params": {
                            "command": "vscode.java.startDebugSession", "arguments": []
                        }})).await?;
                    }
                    LanguageCommand::Classpaths { path, main_class, reply } => {
                        if classpaths.len() >= 8 { let _ = reply.send(Err(LspError("Too many classpath requests pending".into()))); continue; }
                        let id = next_id; next_id += 1;
                        let source_uri = uri(&path)?;
                        classpaths.insert(id, PendingClasspaths { sent: Instant::now(), reply, path, main_class, paths: None });
                        send(input, json!({"jsonrpc": "2.0", "id": id, "method": "workspace/executeCommand", "params": {
                            "command": "java.project.getClasspaths", "arguments": [source_uri, "{\"scope\":\"runtime\"}"]
                        }})).await?;
                    }
                    LanguageCommand::RefreshMaven { root_pom } => {
                        settings = crate::java::lsp_config::java_settings_for_maven_refresh(&settings);
                        notify(input, "workspace/didChangeConfiguration", json!({"settings": settings.clone()})).await?;
                        let id = next_id;
                        next_id += 1;
                        send(input, json!({"jsonrpc": "2.0", "id": id, "method": "workspace/executeCommand", "params": {
                            "command": "java.project.changeImportedProjects",
                            "arguments": maven_import_arguments(&uri(&root_pom)?)
                        }})).await?;
                    }
                    LanguageCommand::Open { path, text, version } => notify(input, "textDocument/didOpen", json!({"textDocument": {"uri": uri(&path)?, "languageId": "java", "version": version, "text": text}})).await?,
                    LanguageCommand::Change { path, text, version } => notify(input, "textDocument/didChange", json!({"textDocument": {"uri": uri(&path)?, "version": version}, "contentChanges": [{"text": text}]})).await?,
                    LanguageCommand::Save(path) => notify(input, "textDocument/didSave", json!({"textDocument": {"uri": uri(&path)?}})).await?,
                    LanguageCommand::Close(path) => notify(input, "textDocument/didClose", json!({"textDocument": {"uri": uri(&path)?}})).await?,
                    LanguageCommand::Request { command, path, project_root, position, document_end, range, diagnostics, version, rename_to, tab_size, insert_spaces } => {
                        let id = next_id; next_id += 1;
                        let mut params = json!({"textDocument": {"uri": uri(&path)?}, "position": position});
                        if let Some(new_name) = &rename_to {
                            params["newName"] = json!(new_name);
                        }
                        if command == EditorCommand::DocumentSymbols { params = json!({"textDocument": {"uri": uri(&path)?}}); }
                        if command == EditorCommand::Format { params = json!({"textDocument": {"uri": uri(&path)?}, "options": {"tabSize": tab_size, "insertSpaces": insert_spaces}}); }
                        if command == EditorCommand::OrganizeImports { params = json!({"textDocument": {"uri": uri(&path)?}, "range": {"start": {"line": 0, "character": 0}, "end": document_end}, "context": {"diagnostics": []}}); }
                        if command == EditorCommand::CodeActions { params = json!({"textDocument": {"uri": uri(&path)?}, "range": range, "context": {"diagnostics": diagnostics}}); }
                        if command == EditorCommand::References { params["context"] = json!({"includeDeclaration": true}); }
                        if pending.len() + pending_symbols.len() >= 32 {
                            if rename_to.is_some() {
                                let _ = events.send(LanguageEvent::RenameEdits {
                                    path, version, position,
                                    result: Err("Too many Java requests pending; try again shortly".into()),
                                }).await;
                            } else {
                                let _ = events.send(LanguageEvent::ServerState("Too many Java requests pending; try again shortly".into())).await;
                            }
                            continue;
                        }
                        pending.insert(id, Pending { command, path, project_root, version, position, rename_to, sent: Instant::now() });
                        send(input, json!({"jsonrpc": "2.0", "id": id, "method": command.method(), "params": params})).await?;
                    }
                    LanguageCommand::SearchSymbols { query } => {
                        let stale: Vec<_> = pending_symbols.keys().copied().collect();
                        for stale_id in stale {
                            pending_symbols.remove(&stale_id);
                            notify(input, "$/cancelRequest", json!({"id": stale_id})).await?;
                        }
                        if query.is_empty() { continue; }
                        if pending.len() + pending_symbols.len() >= 32 {
                            let _ = events.send(LanguageEvent::WorkspaceSymbols {
                                query,
                                result: Err("Java language service is busy; try again shortly".into()),
                            }).await;
                            continue;
                        }
                        let id = next_id;
                        next_id += 1;
                        pending_symbols.insert(id, (query.clone(), Instant::now()));
                        send(input, json!({"jsonrpc": "2.0", "id": id, "method": "workspace/symbol", "params": {"query": query}})).await?;
                    }
                }
            }
            message = messages.recv() => {
                let message = message.ok_or_else(|| LspError("JDT LS disconnected".into()))??;
                if message.get("method").is_some() { server_message(input, &message, &settings, &root_uri, events).await?; }
                else if let Some((_, reply)) = message.get("id").and_then(Value::as_u64).and_then(|id| debug_ports.remove(&id)) {
                    let result = if let Some(error) = message.get("error") { Err(LspError(format!("vscode.java.startDebugSession failed: {error}. Check java-debug/JDT LS compatibility and restart Java intelligence"))) }
                        else { message["result"].as_u64().and_then(|port| u16::try_from(port).ok()).filter(|port| *port != 0)
                            .ok_or_else(|| LspError("Java debug server returned an invalid loopback port".into())) };
                    let _ = reply.send(result);
                }
                else if let Some(mut request) = message.get("id").and_then(Value::as_u64).and_then(|id| classpaths.remove(&id)) {
                    if let Some(error) = message.get("error") {
                        let _ = request.reply.send(Err(LspError(format!("JDT LS runtime project resolution failed: {error}"))));
                    } else if let Some(mut paths) = request.paths.take() {
                        match main_project_name(&message["result"], &request.main_class, &request.path) {
                            Ok(name) => { paths.project_name = name; let _ = request.reply.send(Ok(paths)); }
                            Err(error) => { let _ = request.reply.send(Err(error)); }
                        }
                    } else {
                        match serde_json::from_value::<RuntimeClasspaths>(message["result"].clone()) {
                            Ok(paths) if paths.project_name.as_deref().is_none_or(str::is_empty) && !bundles.is_empty() => {
                                request.paths = Some(paths);
                                let id = next_id; next_id += 1;
                                classpaths.insert(id, request);
                                send(input, json!({"jsonrpc":"2.0", "id":id, "method":"workspace/executeCommand", "params":{"command":"vscode.java.resolveMainClass", "arguments":[]}})).await?;
                            }
                            Ok(paths) => { let _ = request.reply.send(Ok(paths)); }
                            Err(error) => { let _ = request.reply.send(Err(LspError(format!("Invalid runtime classpath response: {error}")))); }
                        }
                    }
                }
                else if let Some((query, _)) = message.get("id").and_then(Value::as_u64).and_then(|id| pending_symbols.remove(&id)) {
                    let result = if let Some(error) = message.get("error") {
                        Err(format!("Java symbol search failed: {error}"))
                    } else {
                        Ok(workspace_symbols(&message["result"], &project.root))
                    };
                    let _ = events.send(LanguageEvent::WorkspaceSymbols { query, result }).await;
                }
                else if let Some(request) = message.get("id").and_then(Value::as_u64).and_then(|id| pending.remove(&id)) {
                    if let Some(error) = message.get("error") {
                        deliver_request_error(request, format!("Java request failed: {error}"), events).await;
                    } else { deliver_response(request, &message["result"], events).await; }
                }
            }
            _ = heartbeat.tick() => {
                let expired_debug: Vec<_> = debug_ports.iter().filter(|(_, (sent, reply))| reply.is_closed() || sent.elapsed() > Duration::from_secs(60)).map(|(id, _)| *id).collect();
                for id in expired_debug {
                    if let Some((_, reply)) = debug_ports.remove(&id) { let _ = reply.send(Err(LspError("Debug server start request timed out or was cancelled".into()))); }
                    notify(input, "$/cancelRequest", json!({"id": id})).await?;
                }
                let expired_paths: Vec<_> = classpaths.iter().filter(|(_, request)| request.reply.is_closed() || request.sent.elapsed() > Duration::from_secs(60)).map(|(id, _)| *id).collect();
                for id in expired_paths {
                    if let Some(request) = classpaths.remove(&id) { let _ = request.reply.send(Err(LspError("Runtime classpath request timed out or was cancelled".into()))); }
                    notify(input, "$/cancelRequest", json!({"id": id})).await?;
                }
                let expired: Vec<_> = pending.iter().filter(|(_, request)| request.sent.elapsed() > Duration::from_secs(30)).map(|(id, _)| *id).collect();
                for id in expired {
                    let request = pending.remove(&id);
                    notify(input, "$/cancelRequest", json!({"id": id})).await?;
                    if let Some(request) = request {
                        deliver_request_error(request, "Java request timed out".into(), events).await;
                    }
                }
                let expired_symbols: Vec<_> = pending_symbols.iter()
                    .filter(|(_, (_, sent))| sent.elapsed() > Duration::from_secs(30))
                    .map(|(id, _)| *id).collect();
                for id in expired_symbols {
                    if let Some((query, _)) = pending_symbols.remove(&id) {
                        let _ = events.send(LanguageEvent::WorkspaceSymbols {
                            query,
                            result: Err("Java symbol search timed out".into()),
                        }).await;
                    }
                    notify(input, "$/cancelRequest", json!({"id": id})).await?;
                }
            }
        }
    }
    let shutdown_id = next_id;
    send(
        input,
        json!({"jsonrpc": "2.0", "id": shutdown_id, "method": "shutdown", "params": null}),
    )
    .await?;
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(message) = messages.recv().await {
            let message = message?;
            if message.get("id").and_then(Value::as_u64) == Some(shutdown_id) {
                break;
            }
            if message.get("method").is_some() {
                server_message(input, &message, &settings, &root_uri, events).await?;
            }
        }
        Ok::<(), LspError>(())
    })
    .await;
    notify(input, "exit", Value::Null).await?;
    Ok(())
}

async fn server_message(
    input: &mut (impl AsyncWrite + Unpin),
    message: &Value,
    settings: &Value,
    root_uri: &str,
    events: &LanguageEventSender,
) -> Result<(), LspError> {
    let method = message["method"].as_str().unwrap_or_default();
    let params = &message["params"];
    if let Some(id) = message.get("id") {
        let result = match method {
            "workspace/configuration" => Value::Array(
                params["items"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .map(|item| {
                                item["section"]
                                    .as_str()
                                    .map(|section| {
                                        section
                                            .split('.')
                                            .fold(settings, |value, key| &value[key])
                                            .clone()
                                    })
                                    .unwrap_or_else(|| settings.clone())
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            "workspace/workspaceFolders" => json!([{"uri": root_uri, "name": "Java project"}]),
            "client/registerCapability"
            | "client/unregisterCapability"
            | "window/workDoneProgress/create" => Value::Null,
            "window/showMessageRequest" => Value::Null,
            "workspace/applyEdit" => {
                json!({"applied": false, "failureReason": "Workspace edits are not supported"})
            }
            _ => {
                send(input, json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "Client method not supported"}})).await?;
                return Ok(());
            }
        };
        send(input, json!({"jsonrpc": "2.0", "id": id, "result": result})).await?;
    } else if method == "textDocument/publishDiagnostics" {
        if let Some(path) = file_path(&params["uri"]) {
            let items = params["diagnostics"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .take(2000)
                        .filter_map(|item| serde_json::from_value(item.clone()).ok())
                        .collect()
                })
                .unwrap_or_default();
            let _ = events
                .send(LanguageEvent::Diagnostics {
                    path,
                    version: params["version"].as_i64(),
                    items,
                })
                .await;
        }
    } else if method == "window/showMessage" || method == "language/status" {
        if let Some(text) = params["message"].as_str() {
            let _ = events
                .send(LanguageEvent::ServerState(text.to_owned()))
                .await;
        }
    }
    Ok(())
}

async fn deliver_response(request: Pending, result: &Value, events: &LanguageEventSender) {
    if request.rename_to.is_some() {
        let result = parse_rename_workspace_edits(&request.project_root, result);
        let _ = events
            .send(LanguageEvent::RenameEdits {
                path: request.path,
                version: request.version,
                position: request.position,
                result,
            })
            .await;
        return;
    }
    let Pending {
        command,
        path,
        version,
        position,
        ..
    } = request;
    let event = match command {
        EditorCommand::Completion => {
            let values = result.as_array().or_else(|| result["items"].as_array());
            let items = values
                .into_iter()
                .flatten()
                .take(200)
                .filter_map(|item| {
                    let label = item["label"].as_str()?.to_owned();
                    let edit = &item["textEdit"];
                    let range = serde_json::from_value(edit["range"].clone())
                        .ok()
                        .or_else(|| serde_json::from_value(edit["replace"].clone()).ok());
                    let insert_text = edit["newText"]
                        .as_str()
                        .or_else(|| item["insertText"].as_str())
                        .unwrap_or(&label)
                        .to_owned();
                    let is_snippet = item["insertTextFormat"].as_u64() == Some(2);
                    let expansion = is_snippet
                        .then(|| SnippetParser::parse(&insert_text))
                        .flatten();
                    let snippet_supported = !is_snippet || expansion.is_some();
                    let (insert_text, snippet_tab_stops) = if is_snippet {
                        match expansion {
                            Some(expansion) => {
                                let mut tab_stops: Vec<_> = expansion
                                    .tab_stops
                                    .into_iter()
                                    .map(|(index, ranges)| (index, ranges))
                                    .collect();
                                tab_stops.sort_by_key(|(index, _)| (*index == 0, *index));
                                (expansion.text, Some(tab_stops))
                            }
                            None => (String::new(), None),
                        }
                    } else {
                        (insert_text, None)
                    };
                    let raw_additional = item["additionalTextEdits"].as_array();
                    let additional_edits: Vec<_> = raw_additional
                        .into_iter()
                        .flatten()
                        .filter_map(|edit| {
                            Some((
                                serde_json::from_value(edit["range"].clone()).ok()?,
                                edit["newText"].as_str()?.to_owned(),
                            ))
                        })
                        .collect();
                    let valid_edits = raw_additional
                        .is_none_or(|edits| edits.len() == additional_edits.len())
                        && (item.get("textEdit").is_none_or(Value::is_null) || range.is_some());
                    Some(CompletionItem {
                        label,
                        detail: item["detail"].as_str().unwrap_or_default().to_owned(),
                        insert_text,
                        range,
                        additional_edits,
                        snippet_tab_stops,
                        applicable: valid_edits
                            && snippet_supported
                            && item.get("command").is_none_or(Value::is_null),
                    })
                })
                .collect();
            LanguageEvent::Completion {
                path,
                version,
                position,
                items,
            }
        }
        EditorCommand::Hover => LanguageEvent::Hover {
            path,
            version,
            text: hover_text(&result["contents"]),
        },
        EditorCommand::SignatureHelp => {
            let signatures = result["signatures"]
                .as_array()
                .into_iter()
                .flatten()
                .take(20)
                .filter_map(|signature| {
                    let raw_label = signature["label"].as_str()?;
                    let label = raw_label.chars().take(4096).collect();
                    let documentation = signature["documentation"]
                        .as_str()
                        .or_else(|| signature["documentation"]["value"].as_str())
                        .unwrap_or_default()
                        .chars()
                        .take(4096)
                        .collect();
                    let active_parameter_index = result["activeParameter"]
                        .as_u64()
                        .or_else(|| signature["activeParameter"].as_u64())
                        .and_then(|index| usize::try_from(index).ok());
                    let active_parameter = active_parameter_index
                        .and_then(|index| signature["parameters"].get(index))
                        .and_then(|parameter| {
                            signature_parameter_label(raw_label, &parameter["label"])
                        });
                    Some(SignatureInformation {
                        label,
                        documentation,
                        active_parameter,
                    })
                })
                .collect::<Vec<_>>();
            let active_signature = result["activeSignature"].as_u64().unwrap_or(0) as usize;
            LanguageEvent::SignatureHelp {
                path,
                version,
                active_signature: active_signature.min(signatures.len().saturating_sub(1)),
                signatures,
            }
        }
        EditorCommand::Definition => LanguageEvent::Definition {
            path,
            version,
            locations: locations(result),
        },
        EditorCommand::TypeDefinition => LanguageEvent::TypeDefinition {
            path,
            version,
            locations: locations(result),
        },
        EditorCommand::Implementation => LanguageEvent::Implementation {
            path,
            version,
            locations: locations(result),
        },
        EditorCommand::References => LanguageEvent::References {
            path,
            version,
            locations: locations(result),
        },
        EditorCommand::CodeActions => {
            let result = parse_code_actions(&path, version, result);
            LanguageEvent::CodeActions {
                path,
                version,
                position,
                result,
            }
        }
        EditorCommand::DocumentSymbols => LanguageEvent::DocumentSymbols {
            result: document_symbols(result, &path),
            path,
            version,
        },
        EditorCommand::Format | EditorCommand::OrganizeImports => {
            let kind = if command == EditorCommand::Format {
                DocumentEditKind::Format
            } else {
                DocumentEditKind::OrganizeImports
            };
            let result = match kind {
                DocumentEditKind::Format => parse_text_edits(result),
                DocumentEditKind::OrganizeImports => parse_workspace_edits(&path, version, result),
            };
            LanguageEvent::DocumentEdits {
                path,
                version,
                kind,
                result,
            }
        }
        EditorCommand::Rename => {
            LanguageEvent::ServerState("Rename response was missing its new name".into())
        }
    };
    let _ = events.send(event).await;
}

fn parse_text_edits(value: &Value) -> Result<Vec<(Range, String)>, String> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    let edits = value
        .as_array()
        .ok_or("JDT LS returned an invalid text edit list")?;
    if edits.len() > 20_000 {
        return Err("JDT LS returned too many text edits".into());
    }
    edits
        .iter()
        .map(|edit| {
            let range = serde_json::from_value(edit["range"].clone())
                .map_err(|error| format!("Invalid text edit range: {error}"))?;
            let text = edit["newText"]
                .as_str()
                .ok_or_else(|| "Text edit has no replacement text".to_owned())?;
            Ok((range, text.to_owned()))
        })
        .collect()
}

fn parse_code_actions(
    path: &Path,
    version: i64,
    value: &Value,
) -> Result<Vec<CodeActionItem>, String> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    let actions = value
        .as_array()
        .ok_or("JDT LS returned an invalid code action list")?;
    let mut result = Vec::new();
    let mut total_edits = 0usize;
    for action in actions.iter().take(100) {
        let Some(title) = action["title"].as_str() else {
            continue;
        };
        if title.is_empty()
            || action
                .get("disabled")
                .is_some_and(|disabled| !disabled.is_null())
            || action
                .get("command")
                .is_some_and(|command| !command.is_null())
        {
            continue;
        }
        let edit = &action["edit"];
        if edit.is_null()
            || edit["changeAnnotations"]
                .as_object()
                .is_some_and(|items| !items.is_empty())
        {
            continue;
        }
        if edit
            .get("changes")
            .is_some_and(|changes| !changes.is_null())
            && edit
                .get("documentChanges")
                .is_some_and(|changes| !changes.is_null())
        {
            continue;
        }
        let mut edits = Vec::new();
        let mut valid = true;
        if let Some(changes) = edit["changes"].as_object() {
            for (document_uri, raw_edits) in changes {
                let Some(target) = file_path(&Value::String(document_uri.clone())) else {
                    valid = false;
                    break;
                };
                if !same_document_path(&target, path) {
                    valid = false;
                    break;
                }
                let Ok(parsed) = parse_text_edits(raw_edits) else {
                    valid = false;
                    break;
                };
                edits.extend(parsed);
            }
        }
        if valid {
            if let Some(changes) = edit["documentChanges"].as_array() {
                for change in changes {
                    if change.get("kind").is_some() {
                        valid = false;
                        break;
                    }
                    let Some(document_uri) = change["textDocument"]["uri"].as_str() else {
                        valid = false;
                        break;
                    };
                    let document_version = &change["textDocument"]["version"];
                    if !document_version.is_null() && document_version.as_i64() != Some(version) {
                        valid = false;
                        break;
                    }
                    let Some(target) = file_path(&Value::String(document_uri.to_owned())) else {
                        valid = false;
                        break;
                    };
                    if !same_document_path(&target, path) {
                        valid = false;
                        break;
                    }
                    let Ok(parsed) = parse_text_edits(&change["edits"]) else {
                        valid = false;
                        break;
                    };
                    edits.extend(parsed);
                }
            }
        }
        if valid && !edits.is_empty() {
            total_edits = total_edits
                .checked_add(edits.len())
                .ok_or("Too many code action edits")?;
            if total_edits > 20_000 {
                return Err("JDT LS returned too many code action edits".into());
            }
            result.push(CodeActionItem {
                title: title.chars().take(256).collect(),
                edits,
            });
        }
    }
    Ok(result)
}

fn same_document_path(candidate: &Path, current: &Path) -> bool {
    candidate == current || candidate.canonicalize().ok().as_deref() == Some(current)
}

fn parse_workspace_edits(
    path: &Path,
    version: i64,
    value: &Value,
) -> Result<Vec<(Range, String)>, String> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    if value
        .get("changes")
        .is_some_and(|changes| !changes.is_null())
        && value
            .get("documentChanges")
            .is_some_and(|changes| !changes.is_null())
    {
        return Err("JDT LS returned both workspace change formats".into());
    }
    let mut edits = Vec::new();
    let mut found = false;
    if let Some(changes) = value["changes"].as_object() {
        for (uri, text_edits) in changes {
            let target = file_path(&Value::String(uri.clone()))
                .ok_or_else(|| format!("JDT LS returned a non-file edit URI: {uri}"))?;
            if target != path {
                return Err(format!(
                    "Organize imports returned edits for another file: {}",
                    target.display()
                ));
            }
            edits.extend(parse_text_edits(text_edits)?);
            found = true;
        }
    }
    if let Some(document_changes) = value["documentChanges"].as_array() {
        for change in document_changes {
            let document = change
                .get("textDocument")
                .ok_or("JDT LS returned an unsupported resource operation")?;
            let document_version = &document["version"];
            if !document_version.is_null() && document_version.as_i64() != Some(version) {
                return Err("JDT LS returned edits for an outdated document version".into());
            }
            let uri = document["uri"]
                .as_str()
                .ok_or("JDT LS returned an unsupported resource operation")?;
            let target = file_path(&Value::String(uri.to_owned()))
                .ok_or_else(|| format!("JDT LS returned a non-file edit URI: {uri}"))?;
            if target != path {
                return Err(format!(
                    "Organize imports returned edits for another file: {}",
                    target.display()
                ));
            }
            edits.extend(parse_text_edits(&change["edits"])?);
            found = true;
        }
    }
    if !found && value.get("changes").is_none() && value.get("documentChanges").is_none() {
        return Err("JDT LS returned an invalid workspace edit".into());
    }
    if edits.len() > 20_000 {
        return Err("JDT LS returned too many workspace edits".into());
    }
    Ok(edits)
}

fn parse_rename_workspace_edits(
    root: &Path,
    value: &Value,
) -> Result<Vec<WorkspaceFileEdits>, String> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    let root = root
        .canonicalize()
        .map_err(|error| format!("Cannot resolve project root: {error}"))?;
    if value
        .get("changes")
        .is_some_and(|changes| !changes.is_null() && !changes.is_object())
    {
        return Err("JDT LS returned an invalid workspace changes map".into());
    }
    if value
        .get("documentChanges")
        .is_some_and(|changes| !changes.is_null() && !changes.is_array())
    {
        return Err("JDT LS returned an invalid document changes list".into());
    }
    let mut by_path: std::collections::BTreeMap<PathBuf, WorkspaceFileEdits> =
        std::collections::BTreeMap::new();
    let mut resource_renames = Vec::new();
    let mut total_edits = 0usize;
    let mut append =
        |uri_value: &Value, edits_value: &Value, version: Option<i64>| -> Result<(), String> {
            let raw_uri = uri_value
                .as_str()
                .ok_or("JDT LS returned an edit without a document URI")?;
            let path = file_path(&Value::String(raw_uri.to_owned()))
                .ok_or_else(|| format!("JDT LS returned a non-file edit URI: {raw_uri}"))?;
            let canonical = path.canonicalize().map_err(|error| {
                format!("Cannot resolve rename target {}: {error}", path.display())
            })?;
            let metadata = std::fs::metadata(&canonical).map_err(|error| {
                format!(
                    "Cannot inspect rename target {}: {error}",
                    canonical.display()
                )
            })?;
            if !metadata.is_file() || !canonical.starts_with(&root) {
                return Err(format!(
                    "Rename target must be a regular file inside the open project: {}",
                    canonical.display()
                ));
            }
            let edits = parse_text_edits(edits_value)?;
            total_edits = total_edits
                .checked_add(edits.len())
                .ok_or("Too many rename edits")?;
            if total_edits > 20_000 {
                return Err("JDT LS returned too many rename edits".into());
            }
            let entry = by_path
                .entry(canonical.clone())
                .or_insert_with(|| WorkspaceFileEdits {
                    path: canonical,
                    edits: Vec::new(),
                    rename_to: None,
                    version,
                });
            if entry.version != version {
                return Err(format!(
                    "JDT LS returned inconsistent document versions for {}",
                    entry.path.display()
                ));
            }
            entry.edits.extend(edits);
            if by_path.len() > 200 {
                return Err(
                    "Rename affects more than 200 files; narrow the symbol scope and try again"
                        .into(),
                );
            }
            Ok(())
        };
    if let Some(changes) = value["changes"].as_object() {
        for (uri, edits) in changes {
            append(&Value::String(uri.clone()), edits, None)?;
        }
    }
    if let Some(changes) = value["documentChanges"].as_array() {
        for change in changes {
            if change.get("kind").is_some() {
                if change["kind"].as_str() != Some("rename") {
                    return Err("JDT LS returned an unsupported resource operation".into());
                }
                let old_uri = change["oldUri"]
                    .as_str()
                    .ok_or("JDT LS returned a rename without oldUri")?;
                let new_uri = change["newUri"]
                    .as_str()
                    .ok_or("JDT LS returned a rename without newUri")?;
                let old_path = file_path(&Value::String(old_uri.to_owned()))
                    .ok_or("JDT LS returned a non-file rename source")?
                    .canonicalize()
                    .map_err(|error| format!("Cannot resolve rename source: {error}"))?;
                let new_path = file_path(&Value::String(new_uri.to_owned()))
                    .ok_or("JDT LS returned a non-file rename destination")?;
                let parent = new_path
                    .parent()
                    .ok_or("Rename destination has no parent")?
                    .canonicalize()
                    .map_err(|error| {
                        format!("Cannot resolve rename destination directory: {error}")
                    })?;
                let destination = parent.join(
                    new_path
                        .file_name()
                        .ok_or("Rename destination has no filename")?,
                );
                let destination_exists = match std::fs::symlink_metadata(&destination) {
                    Ok(_) => true,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                    Err(error) => {
                        return Err(format!(
                            "Cannot inspect rename destination {}: {error}",
                            destination.display()
                        ));
                    }
                };
                if !old_path.starts_with(&root)
                    || parent != old_path.parent().unwrap_or(Path::new(""))
                    || old_path.extension().is_none_or(|ext| ext != "java")
                    || destination.extension().is_none_or(|ext| ext != "java")
                    || destination_exists
                {
                    return Err("Java source rename must stay in the same project directory and cannot overwrite a file".into());
                }
                resource_renames.push((old_path, destination));
            } else {
                let document = change
                    .get("textDocument")
                    .ok_or("JDT LS returned an invalid document change")?;
                let version = match &document["version"] {
                    Value::Null => None,
                    value => Some(
                        value
                            .as_i64()
                            .ok_or("JDT LS returned an invalid document version")?,
                    ),
                };
                append(&document["uri"], &change["edits"], version)?;
            }
        }
    }
    if value.get("changes").is_none() && value.get("documentChanges").is_none() {
        return Err("JDT LS returned an invalid workspace edit".into());
    }
    drop(append);
    for (old_path, destination) in resource_renames {
        let entry = by_path
            .entry(old_path.clone())
            .or_insert_with(|| WorkspaceFileEdits {
                path: old_path,
                edits: Vec::new(),
                rename_to: None,
                version: None,
            });
        if entry.rename_to.replace(destination).is_some() {
            return Err("JDT LS returned multiple file renames for one source".into());
        }
    }
    Ok(by_path.into_values().collect())
}

fn signature_parameter_label(signature_label: &str, parameter_label: &Value) -> Option<String> {
    if let Some(label) = parameter_label.as_str() {
        return Some(label.chars().take(256).collect());
    }
    let offsets = parameter_label.as_array()?;
    if offsets.len() != 2 {
        return None;
    }
    let start = usize::try_from(offsets[0].as_u64()?).ok()?;
    let end = usize::try_from(offsets[1].as_u64()?).ok()?;
    if end < start {
        return None;
    }
    let start_byte = utf16_offset_to_byte(signature_label, start)?;
    let end_byte = utf16_offset_to_byte(signature_label, end)?;
    signature_label
        .get(start_byte..end_byte)
        .map(|label| label.chars().take(256).collect())
}

fn utf16_offset_to_byte(text: &str, offset: usize) -> Option<usize> {
    let mut units = 0;
    for (byte, character) in text.char_indices() {
        if units == offset {
            return Some(byte);
        }
        units = units.checked_add(character.len_utf16())?;
        if units > offset {
            return None;
        }
    }
    (units == offset).then_some(text.len())
}

async fn deliver_request_error(request: Pending, error: String, events: &LanguageEventSender) {
    if request.rename_to.is_some() {
        let _ = events
            .send(LanguageEvent::RenameEdits {
                path: request.path,
                version: request.version,
                position: request.position,
                result: Err(error),
            })
            .await;
        return;
    }
    if matches!(
        request.command,
        EditorCommand::Format | EditorCommand::OrganizeImports
    ) {
        let kind = if request.command == EditorCommand::Format {
            DocumentEditKind::Format
        } else {
            DocumentEditKind::OrganizeImports
        };
        let _ = events
            .send(LanguageEvent::DocumentEdits {
                path: request.path,
                version: request.version,
                kind,
                result: Err(error),
            })
            .await;
    } else if request.command == EditorCommand::DocumentSymbols {
        let _ = events
            .send(LanguageEvent::DocumentSymbols {
                path: request.path,
                version: request.version,
                result: Err(error),
            })
            .await;
    } else if request.command == EditorCommand::CodeActions {
        let _ = events
            .send(LanguageEvent::CodeActions {
                path: request.path,
                version: request.version,
                position: request.position,
                result: Err(error),
            })
            .await;
    } else {
        let _ = events.send(LanguageEvent::ServerState(error)).await;
    }
}
fn hover_text(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        text.to_owned()
    } else if let Some(items) = value.as_array() {
        items
            .iter()
            .map(hover_text)
            .collect::<Vec<_>>()
            .join("\n\n")
    } else {
        value["value"].as_str().unwrap_or_default().to_owned()
    }
}
fn locations(value: &Value) -> Vec<Location> {
    let items: Vec<_> = if let Some(items) = value.as_array() {
        items.iter().collect()
    } else if value.is_object() {
        vec![value]
    } else {
        Vec::new()
    };
    items
        .into_iter()
        .take(2000)
        .filter_map(|item| {
            Some(Location {
                path: file_path(item.get("uri").unwrap_or(&item["targetUri"]))?,
                range: serde_json::from_value(
                    item.get("range")
                        .unwrap_or(&item["targetSelectionRange"])
                        .clone(),
                )
                .ok()?,
            })
        })
        .collect()
}
fn workspace_symbols(value: &Value, root: &Path) -> Vec<WorkspaceSymbol> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .take(1000)
        .filter_map(|item| {
            let name = item["name"].as_str()?.trim();
            if name.is_empty() {
                return None;
            }
            let location = &item["location"];
            let path = file_path(location.get("uri")?)?;
            if !path.starts_with(root)
                || path.extension().is_none_or(|extension| extension != "java")
            {
                return None;
            }
            let range = serde_json::from_value(location.get("range")?.clone()).ok()?;
            Some(WorkspaceSymbol {
                name: name.to_owned(),
                kind: item["kind"].as_u64().unwrap_or(13),
                container: item["containerName"].as_str().map(str::to_owned),
                location: Location { path, range },
            })
        })
        .take(500)
        .collect()
}

fn document_symbols(value: &Value, path: &Path) -> Result<Vec<OutlineSymbol>, String> {
    let Some(items) = value.as_array() else {
        return if value.is_null() {
            Ok(Vec::new())
        } else {
            Err("JDT LS returned an invalid document symbol list".to_owned())
        };
    };
    let mut symbols = Vec::new();
    fn collect(items: &[Value], path: &Path, depth: usize, output: &mut Vec<OutlineSymbol>) {
        if depth > 32 || output.len() >= 5000 {
            return;
        }
        for item in items {
            if output.len() >= 5000 {
                break;
            }
            let Some(name) = item["name"].as_str().filter(|name| !name.trim().is_empty()) else {
                continue;
            };
            let location = if let Some(location) = item.get("location") {
                location.get("uri").and_then(file_path).and_then(|path| {
                    serde_json::from_value(location.get("range")?.clone())
                        .ok()
                        .map(|range| Location { path, range })
                })
            } else {
                item.get("selectionRange")
                    .or_else(|| item.get("range"))
                    .and_then(|range| serde_json::from_value(range.clone()).ok())
                    .map(|range| Location {
                        path: path.to_path_buf(),
                        range,
                    })
            };
            if let Some(location) = location {
                output.push(OutlineSymbol {
                    name: name.to_owned(),
                    kind: item["kind"].as_u64().unwrap_or(13),
                    location,
                    depth,
                });
            }
            if let Some(children) = item["children"].as_array() {
                collect(children, path, depth + 1, output);
            }
        }
    }
    collect(items, path, 0, &mut symbols);
    Ok(symbols)
}

struct SyncedDocument {
    text: String,
    version: i64,
}

#[derive(Default)]
pub struct LanguageTool {
    session: Option<LanguageSession>,
    retiring: Vec<LanguageSession>,
    pending_start: Option<(
        ProjectModel,
        std::sync::mpsc::Receiver<Result<JdtlsConfig, LspError>>,
    )>,
    tooling_rx: Option<std::sync::mpsc::Receiver<ToolingSnapshot>>,
    pub tooling: ToolingSnapshot,
    repaint_context: Option<eframe::egui::Context>,
    documents: HashMap<PathBuf, SyncedDocument>,
    pending_saves: std::collections::HashSet<PathBuf>,
    pending_maven_files: std::collections::HashSet<PathBuf>,
    maven_import_requested: bool,
    next_version: i64,
    generation: u64,
    pub diagnostics: HashMap<PathBuf, Vec<LanguageDiagnostic>>,
    pub summary: String,
    pub lombok_warning: Option<String>,
    lombok_cache_check: Option<std::sync::mpsc::Receiver<(u64, PathBuf, bool)>>,
    lombok_restart_requested: bool,
    pub code_actions: Option<(PathBuf, i64, Position, Vec<CodeActionItem>)>,
    pub code_actions_error: Option<(PathBuf, i64, String)>,
    pub completion: Option<(PathBuf, i64, Position, Vec<CompletionItem>)>,
    pub completion_selection: Option<usize>,
    pub signature_help: Option<(PathBuf, i64, Vec<SignatureInformation>, usize)>,
    pub hover: Option<(PathBuf, String)>,
    pub locations: Vec<Location>,
    pub locations_title: String,
    pub root: Option<PathBuf>,
    symbol_query_pending: Option<(String, Instant)>,
    pub workspace_symbol_query: String,
    pub workspace_symbols: Vec<WorkspaceSymbol>,
    pub workspace_symbols_loading: bool,
    pub workspace_symbols_error: Option<String>,
    pub document_symbols_path: Option<PathBuf>,
    pub document_symbols_version: Option<i64>,
    pub document_symbols: Vec<OutlineSymbol>,
    pub document_symbols_loading: bool,
    pub document_symbols_error: Option<String>,
}

#[cfg(test)]
mod maven_refresh_tests {
    use std::path::PathBuf;

    use super::LanguageTool;

    #[test]
    fn refresh_waits_for_language_session_and_deduplicates_pom_paths() {
        let mut language = LanguageTool::default();
        let pom = PathBuf::from("/project/pom.xml");
        language
            .refresh_maven_project(vec![pom.clone(), pom.clone()])
            .unwrap();

        assert_eq!(language.pending_maven_files.len(), 1);
        assert!(language.pending_maven_files.contains(&pom));
    }

    #[test]
    fn refresh_rejects_non_pom_paths() {
        let mut language = LanguageTool::default();
        assert!(
            language
                .refresh_maven_project(vec![PathBuf::from("/project/src/Main.java")])
                .is_err()
        );
        assert!(language.pending_maven_files.is_empty());
    }

    #[test]
    fn targeted_import_arguments_include_only_the_reactor_root_pom() {
        assert_eq!(
            super::maven_import_arguments("file:///project/pom.xml"),
            serde_json::json!([["file:///project/pom.xml"], [], []])
        );
    }
}
#[derive(Clone, Debug, Default)]
pub struct ToolingSnapshot {
    pub resolving: bool,
    pub jdtls_path: Option<PathBuf>,
    pub jdtls_error: Option<String>,
    pub debug_bundle: Option<PathBuf>,
    pub debug_error: Option<String>,
}
impl LanguageTool {
    pub fn can_request_for(&self, path: &Path) -> bool {
        self.session.is_some()
            && self
                .root
                .as_ref()
                .is_some_and(|root| path.starts_with(root))
    }

    pub fn set_repaint_context(&mut self, context: eframe::egui::Context) {
        self.repaint_context = Some(context);
    }
    pub fn refresh_tooling(&mut self, settings: &crate::settings::UserSettings) {
        let settings = settings.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        self.tooling.resolving = true;
        self.tooling.jdtls_path = None;
        self.tooling.jdtls_error = None;
        self.tooling.debug_bundle = None;
        self.tooling.debug_error = None;
        self.tooling_rx = Some(receiver);
        let repaint_context = self.repaint_context.clone();
        let _ = std::thread::Builder::new()
            .name("java-tool-discovery".into())
            .spawn(move || {
                let result = std::env::current_exe()
                    .map_err(|error| {
                        LspError(format!("Cannot locate Intelli-gen executable: {error}"))
                    })
                    .and_then(|exe| crate::java::lsp_config::resolve_jdtls(&settings, &exe));
                let (jdtls_path, jdtls_error) = match result {
                    Ok(tools) => (Some(tools.display_path), None),
                    Err(error) => (None, Some(error.to_string())),
                };
                let debug = std::env::current_exe()
                    .map_err(|error| {
                        LspError(format!("Cannot locate Intelli-gen executable: {error}"))
                    })
                    .and_then(|exe| crate::java::lsp_config::resolve_debug_bundle(&settings, &exe))
                    .and_then(|path| {
                        crate::debugger::validate_bundle(&path)
                            .map_err(|error| LspError(error.to_string()))
                    });
                let (debug_bundle, debug_error) = match debug {
                    Ok(path) => (Some(path), None),
                    Err(error) => (None, Some(error.to_string())),
                };
                if sender
                    .send(ToolingSnapshot {
                        resolving: false,
                        jdtls_path,
                        jdtls_error,
                        debug_bundle,
                        debug_error,
                    })
                    .is_ok()
                {
                    if let Some(context) = repaint_context {
                        context.request_repaint();
                    }
                }
            });
    }
    pub fn debug_adapter_resolver(&self) -> Result<DebugAdapterResolver, LspError> {
        let session = self.session.as_ref().ok_or_else(|| {
            LspError("Wait for Java intelligence to start before debugging".into())
        })?;
        let root = self
            .root
            .clone()
            .ok_or_else(|| LspError("Open a Java project first".into()))?;
        Ok(DebugAdapterResolver {
            commands: session.commands.clone(),
            root,
            bundle: session.debug_bundle.clone(),
        })
    }
    pub fn classpath_resolver(&self) -> Result<ClasspathResolver, LspError> {
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| LspError("Wait for Java intelligence to start before running".into()))?;
        let root = self
            .root
            .clone()
            .ok_or_else(|| LspError("Open a Java project first".into()))?;
        Ok(ClasspathResolver {
            commands: session.commands.clone(),
            root,
        })
    }
    pub fn restart(
        &mut self,
        project: Option<&ProjectModel>,
        settings: &crate::settings::UserSettings,
        selection_root: Option<&Path>,
    ) {
        self.shutdown();
        self.lombok_warning = None;
        self.pending_maven_files.clear();
        self.documents.clear();
        self.pending_saves.clear();
        self.diagnostics.clear();
        self.completion = None;
        self.completion_selection = None;
        self.signature_help = None;
        self.hover = None;
        self.locations.clear();
        self.workspace_symbol_query.clear();
        self.workspace_symbols.clear();
        self.workspace_symbols_error = None;
        self.workspace_symbols_loading = false;
        self.document_symbols_path = None;
        self.document_symbols_version = None;
        self.document_symbols.clear();
        self.document_symbols_loading = false;
        self.document_symbols_error = None;
        self.code_actions = None;
        self.code_actions_error = None;
        self.root = project.map(|project| project.root.clone());
        let Some(project) =
            project.filter(|project| project.kind != crate::project::ProjectKind::Unknown)
        else {
            self.summary = "Java intelligence: open a Maven or Gradle project".into();
            return;
        };
        let settings = settings.clone();
        let root = project.root.clone();
        let selected_root = selection_root.map(Path::to_path_buf);
        let (sender, receiver) = std::sync::mpsc::channel();
        self.pending_start = Some((project.clone(), receiver));
        self.summary = "Discovering Java tooling…".into();
        let repaint_context = self.repaint_context.clone();
        let _ = std::thread::Builder::new()
            .name("java-config-discovery".into())
            .spawn(move || {
                let result = JdtlsConfig::from_settings(&settings, &root).map(|mut config| {
                    if let Some(root) = selected_root.as_deref() {
                        config.project_jdk_id = settings
                            .project_jdk_id(root)
                            .map(str::to_owned)
                            .or_else(|| settings.default_jdk_id.clone());
                    }
                    config
                });
                if sender.send(result).is_ok() {
                    if let Some(context) = repaint_context {
                        context.request_repaint();
                    }
                }
            });
    }
    pub fn shutdown(&mut self) {
        // Invalidate worker events even if the app already queued them before
        // a restart/project switch/quit was reduced in the same frame.
        self.generation = self
            .generation
            .checked_add(1)
            .expect("Language generation exhausted");
        self.pending_start = None;
        self.lombok_cache_check = None;
        self.lombok_restart_requested = false;
        self.pending_maven_files.clear();
        self.maven_import_requested = false;
        self.symbol_query_pending = None;
        self.workspace_symbols_loading = false;
        if let Some(session) = self.session.take() {
            session.shutdown();
            self.retiring.push(session);
        }
        for session in &self.retiring {
            session.shutdown();
        }
    }
    pub fn has_workers(&self) -> bool {
        self.session.is_some() || !self.retiring.is_empty()
    }
    pub fn has_pending_start(&self) -> bool {
        self.pending_start.is_some()
    }
    pub fn check_lombok_after_successful_build(&mut self) {
        if self.lombok_warning.is_none() || self.lombok_cache_check.is_some() {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let generation = self.generation;
        let (sender, receiver) = std::sync::mpsc::channel();
        self.lombok_cache_check = Some(receiver);
        let repaint_context = self.repaint_context.clone();
        if std::thread::Builder::new()
            .name("lombok-cache-check".into())
            .spawn(move || {
                let available = crate::java::lsp_config::project_lombok_agent_is_cached(&root);
                if sender.send((generation, root, available)).is_ok()
                    && let Some(context) = repaint_context
                {
                    context.request_repaint();
                }
            })
            .is_err()
        {
            self.lombok_cache_check = None;
        }
    }
    pub fn take_lombok_restart_request(&mut self) -> bool {
        std::mem::take(&mut self.lombok_restart_requested)
    }
    pub fn poll(&mut self) -> Vec<LanguageUpdate> {
        if let Some(receiver) = self.lombok_cache_check.take() {
            match receiver.try_recv() {
                Ok((generation, root, available)) => {
                    if available
                        && generation == self.generation
                        && self.root.as_ref() == Some(&root)
                        && self.lombok_warning.is_some()
                    {
                        self.lombok_restart_requested = true;
                        self.summary =
                            "Lombok dependency is cached; restarting Java intelligence…".into();
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.lombok_cache_check = Some(receiver);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
            }
        }
        if let Some(receiver) = self.tooling_rx.take() {
            match receiver.try_recv() {
                Ok(snapshot) => self.tooling = snapshot,
                Err(std::sync::mpsc::TryRecvError::Empty) => self.tooling_rx = Some(receiver),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.tooling.resolving = false,
            }
        }
        self.retiring.retain_mut(|session| {
            for _ in 0..128 {
                if session.events.try_recv().is_err() {
                    break;
                }
            }
            session.reap().is_none()
        });
        if self.retiring.is_empty() && self.session.is_none() {
            if let Some((project, receiver)) = self.pending_start.take() {
                match receiver.try_recv() {
                    Ok(Ok(config)) => match JavaLanguageService::start_for_generation(
                        &project,
                        &config,
                        self.generation,
                    ) {
                        Ok(session) => {
                            self.lombok_warning = (config.lombok_declared && config.lombok_agent.is_none()).then(||
                                "Lombok is declared, but its JAR is not in the local Maven/Gradle cache. Resolve it with a successful build or restart Java intelligence after resolving it elsewhere.".to_owned()
                            );
                            self.session = Some(session);
                        }
                        Err(error) => {
                            self.summary = format!("Java intelligence unavailable: {error}")
                        }
                    },
                    Ok(Err(error)) => {
                        self.summary = format!("Java intelligence unavailable: {error}")
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        self.pending_start = Some((project, receiver))
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.summary = "Java tooling discovery worker stopped".into()
                    }
                }
            }
        }
        let mut events = Vec::new();
        let Some(session) = &mut self.session else {
            if self.pending_start.is_none()
                && let Some((query, _)) = self.symbol_query_pending.take()
            {
                if !query.trim().is_empty() {
                    self.workspace_symbols_loading = false;
                    self.workspace_symbols_error = Some(self.summary.clone());
                }
            }
            return events;
        };
        if !self.pending_maven_files.is_empty() && !self.maven_import_requested {
            let root_pom = maven_root_pom(&self.pending_maven_files, self.root.as_deref());
            if let Some(root_pom) = root_pom
                && session
                    .try_command(LanguageCommand::RefreshMaven { root_pom })
                    .is_ok()
            {
                self.pending_maven_files.clear();
                self.maven_import_requested = true;
            }
        }
        if self
            .symbol_query_pending
            .as_ref()
            .is_some_and(|(_, requested)| requested.elapsed() >= Duration::from_millis(220))
        {
            if let Some((query, _)) = self.symbol_query_pending.take() {
                if query.is_empty() {
                    self.workspace_symbols_loading = false;
                } else {
                    match session.try_command(LanguageCommand::SearchSymbols { query }) {
                        Ok(()) => self.workspace_symbols_loading = true,
                        Err(error) => {
                            self.workspace_symbols_loading = false;
                            self.workspace_symbols_error = Some(error.to_string());
                        }
                    }
                }
            }
        }
        for _ in 0..128 {
            match session.events.try_recv() {
                Ok(event) => events.push(event),
                Err(_) => break,
            }
        }
        if let Some(result) = session.reap() {
            // Drain final queued status before removing the session.
            while let Ok(event) = session.events.try_recv() {
                events.push(event);
            }
            if let Err(error) = result {
                events.push(LanguageUpdate {
                    generation: session.generation,
                    event: LanguageEvent::ServerState(error),
                });
            }
            self.session = None;
        }
        events
    }
    pub fn refresh_maven_project(&mut self, pom_files: Vec<PathBuf>) -> Result<(), LspError> {
        if pom_files
            .iter()
            .any(|path| path.file_name().is_none_or(|name| name != "pom.xml"))
        {
            return Err(LspError("Maven refresh accepts only pom.xml paths".into()));
        }
        self.pending_maven_files.extend(pom_files);
        if let Some(session) = &self.session {
            let root_pom = maven_root_pom(&self.pending_maven_files, self.root.as_deref())
                .ok_or_else(|| LspError("Cannot locate the Maven reactor root pom.xml".into()))?;
            session.try_command(LanguageCommand::RefreshMaven { root_pom })?;
            self.pending_maven_files.clear();
            self.maven_import_requested = true;
        }
        Ok(())
    }
    pub fn search_workspace_symbols(&mut self, query: String) {
        if query == self.workspace_symbol_query {
            return;
        }
        self.workspace_symbol_query = query.clone();
        self.workspace_symbols.clear();
        self.workspace_symbols_error = None;
        self.workspace_symbols_loading = !query.trim().is_empty();
        let pending_query = if query.trim().is_empty() {
            String::new()
        } else {
            query
        };
        self.symbol_query_pending = Some((pending_query, Instant::now()));
    }
    pub fn request_document_symbols(&mut self, document: &Document) {
        self.document_symbols_path = Some(document.path.clone());
        self.document_symbols.clear();
        self.document_symbols_error = None;
        self.document_symbols_loading = false;
        let result = (|| {
            let session = self
                .session
                .as_ref()
                .ok_or_else(|| LspError(self.summary.clone()))?;
            let synced = self
                .documents
                .get(&document.path)
                .filter(|synced| synced.text == document.text)
                .ok_or_else(|| {
                    LspError("Java document is not synchronized yet; try again shortly".into())
                })?;
            session.try_command(LanguageCommand::Request {
                command: EditorCommand::DocumentSymbols,
                path: document.path.clone(),
                project_root: self
                    .root
                    .clone()
                    .ok_or_else(|| LspError("No Java workspace is open".into()))?,
                position: document.lsp_position(),
                document_end: document_end_position(&document.text),
                range: document_line_range(&document.text, document.lsp_position().line),
                diagnostics: Vec::new(),
                version: synced.version,
                rename_to: None,
                tab_size: 4,
                insert_spaces: true,
            })?;
            self.document_symbols_version = Some(synced.version);
            Ok::<(), LspError>(())
        })();
        match result {
            Ok(()) => self.document_symbols_loading = true,
            Err(error) => self.document_symbols_error = Some(error.to_string()),
        }
    }
    pub fn did_save(&mut self, path: PathBuf) {
        if self.documents.contains_key(&path) {
            self.pending_saves.insert(path);
        }
    }
    /// Called before requests and after rendering. Track exactly the snapshots
    /// successfully enqueued, and retry on a full command queue next frame.
    pub fn sync_documents(&mut self, tabs: &[crate::editor::EditorTab]) {
        let Some(session) = &self.session else {
            return;
        };
        let eligible = |path: &Path| {
            path.extension()
                .is_some_and(|extension| extension == "java")
                && self
                    .root
                    .as_ref()
                    .is_some_and(|root| path.starts_with(root))
        };
        // Send saves even when Save-and-Close removed the tab in the same
        // reducer pass. didSave must precede the corresponding didClose.
        let saved: Vec<_> = self.pending_saves.iter().cloned().collect();
        for path in saved {
            if session
                .try_command(LanguageCommand::Save(path.clone()))
                .is_ok()
            {
                self.pending_saves.remove(&path);
            }
        }
        let closed: Vec<_> = self
            .documents
            .keys()
            .filter(|path| {
                !tabs
                    .iter()
                    .any(|tab| &tab.path == *path && eligible(&tab.path))
            })
            .cloned()
            .collect();
        for path in closed {
            if !self.pending_saves.contains(&path)
                && session
                    .try_command(LanguageCommand::Close(path.clone()))
                    .is_ok()
            {
                self.documents.remove(&path);
                self.diagnostics.remove(&path);
            }
        }
        for tab in tabs.iter().filter(|tab| eligible(&tab.path)) {
            let document = &tab.document;
            if let Some(previous) = self.documents.get_mut(&tab.path) {
                if previous.text != document.text {
                    let version = self.next_version + 1;
                    if session
                        .try_command(LanguageCommand::Change {
                            path: tab.path.clone(),
                            text: document.text.clone(),
                            version,
                        })
                        .is_err()
                    {
                        continue;
                    }
                    self.next_version = version;
                    previous.version = version;
                    previous.text.clone_from(&document.text);
                    self.diagnostics.remove(&tab.path);
                    self.code_actions = None;
                    self.code_actions_error = None;
                    self.completion = None;
                    self.completion_selection = None;
                    self.signature_help = None;
                    self.hover = None;
                    if self.document_symbols_path.as_ref() == Some(&tab.path) {
                        self.document_symbols.clear();
                        self.document_symbols_loading = false;
                        self.document_symbols_error = Some(
                            "Source changed. Reopen File Structure to refresh symbols.".into(),
                        );
                    }
                }
            } else {
                let version = self.next_version + 1;
                if session
                    .try_command(LanguageCommand::Open {
                        path: tab.path.clone(),
                        text: document.text.clone(),
                        version,
                    })
                    .is_ok()
                {
                    self.next_version = version;
                    self.documents.insert(
                        tab.path.clone(),
                        SyncedDocument {
                            text: document.text.clone(),
                            version,
                        },
                    );
                }
            }
        }
    }
    pub fn request(
        &mut self,
        command: EditorCommand,
        document: &Document,
        tab_size: usize,
        insert_spaces: bool,
    ) {
        if command == EditorCommand::CodeActions {
            self.code_actions = None;
            self.code_actions_error = None;
        }
        let result = (|| {
            let session = self
                .session
                .as_ref()
                .ok_or_else(|| LspError(self.summary.clone()))?;
            let synced = self
                .documents
                .get(&document.path)
                .filter(|synced| synced.text == document.text)
                .ok_or_else(|| {
                    LspError("Java document is not synchronized yet; try again shortly".into())
                })?;
            session.try_command(LanguageCommand::Request {
                command,
                path: document.path.clone(),
                project_root: self
                    .root
                    .clone()
                    .ok_or_else(|| LspError("No Java workspace is open".into()))?,
                position: document.lsp_position(),
                document_end: document_end_position(&document.text),
                range: document_line_range(&document.text, document.lsp_position().line),
                diagnostics: if command == EditorCommand::CodeActions {
                    self.diagnostics
                        .get(&document.path)
                        .into_iter()
                        .flatten()
                        .filter(|diagnostic| {
                            diagnostic.range.start.line <= document.lsp_position().line
                                && diagnostic.range.end.line >= document.lsp_position().line
                        })
                        .cloned()
                        .collect()
                } else {
                    Vec::new()
                },
                version: synced.version,
                rename_to: None,
                tab_size: tab_size.clamp(1, 8),
                insert_spaces,
            })
        })();
        if let Err(error) = result {
            self.summary = error.to_string();
        }
    }
    pub fn request_rename(
        &mut self,
        document: &Document,
        position: Position,
        new_name: String,
    ) -> Result<(), LspError> {
        let result = (|| {
            if new_name.trim().is_empty() || new_name.chars().count() > 256 {
                return Err(LspError(
                    "Enter a nonempty Java name of at most 256 characters".into(),
                ));
            }
            if document.lsp_position() != position {
                return Err(LspError(
                    "The rename location changed; invoke Rename Symbol again".into(),
                ));
            }
            let session = self
                .session
                .as_ref()
                .ok_or_else(|| LspError(self.summary.clone()))?;
            let synced = self
                .documents
                .get(&document.path)
                .filter(|synced| synced.text == document.text)
                .ok_or_else(|| {
                    LspError("Java document is not synchronized yet; try again shortly".into())
                })?;
            let project_root = self
                .root
                .clone()
                .ok_or_else(|| LspError("No Java workspace is open".into()))?;
            session.try_command(LanguageCommand::Request {
                command: EditorCommand::Rename,
                path: document.path.clone(),
                project_root,
                position,
                document_end: document_end_position(&document.text),
                range: document_line_range(&document.text, position.line),
                diagnostics: Vec::new(),
                version: synced.version,
                rename_to: Some(new_name.trim().to_owned()),
                tab_size: 4,
                insert_spaces: true,
            })
        })();
        if let Err(error) = &result {
            self.summary = error.to_string();
        }
        result
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn document_versions(&self) -> std::collections::HashMap<PathBuf, i64> {
        self.documents
            .iter()
            .map(|(path, document)| (path.clone(), document.version))
            .collect()
    }
    pub fn accepts_generation(&self, generation: u64) -> bool {
        self.generation == generation
    }
    pub fn is_document_current(&self, path: &Path, version: i64, text: &str) -> bool {
        self.current(path, version)
            && self
                .documents
                .get(path)
                .is_some_and(|document| document.text == text)
    }
    fn current(&self, path: &Path, version: i64) -> bool {
        self.documents
            .get(path)
            .is_some_and(|document| document.version == version)
    }
    pub fn receive(&mut self, update: LanguageUpdate) {
        if update.generation != self.generation {
            return;
        }
        match update.event {
            LanguageEvent::ServerState(message) => self.summary = message,
            LanguageEvent::Diagnostics {
                path,
                version,
                items,
            } => {
                if self
                    .root
                    .as_ref()
                    .is_some_and(|root| path.starts_with(root))
                    && version.is_none_or(|version| self.current(&path, version))
                {
                    if self.diagnostics.len() < 1000 || self.diagnostics.contains_key(&path) {
                        self.diagnostics.insert(path, items);
                    }
                }
            }
            LanguageEvent::Completion {
                path,
                version,
                position,
                items,
            } if self.current(&path, version) => {
                self.completion_selection = items.iter().position(|item| item.applicable);
                self.completion = Some((path, version, position, items));
            }
            LanguageEvent::CodeActions {
                path,
                version,
                position,
                result,
            } if self.current(&path, version) => match result {
                Ok(actions) => {
                    self.code_actions = Some((path, version, position, actions));
                    self.code_actions_error = None;
                }
                Err(error) => self.code_actions_error = Some((path, version, error)),
            },
            LanguageEvent::SignatureHelp {
                path,
                version,
                signatures,
                active_signature,
            } if self.current(&path, version) => {
                self.signature_help = Some((path, version, signatures, active_signature));
            }
            LanguageEvent::Hover {
                path,
                version,
                text,
            } if self.current(&path, version) => {
                self.hover = Some((
                    path,
                    if text.is_empty() {
                        "No documentation at this position".into()
                    } else {
                        text
                    },
                ))
            }
            LanguageEvent::Definition {
                path,
                version,
                locations,
            } if self.current(&path, version) => {
                self.locations = locations;
                self.locations_title = "Definitions".into();
                if self.locations.is_empty() {
                    self.summary = "No file-backed definition found".into();
                }
            }
            LanguageEvent::Implementation {
                path,
                version,
                locations,
            } if self.current(&path, version) => {
                self.locations = locations;
                self.locations_title = "Implementations".into();
                if self.locations.is_empty() {
                    self.summary = "No file-backed implementation found".into();
                }
            }
            LanguageEvent::TypeDefinition {
                path,
                version,
                locations,
            } if self.current(&path, version) => {
                self.locations = locations;
                self.locations_title = "Type Definitions".into();
                if self.locations.is_empty() {
                    self.summary = "No file-backed type definition found".into();
                }
            }
            LanguageEvent::References {
                path,
                version,
                locations,
            } if self.current(&path, version) => {
                self.locations = locations;
                self.locations_title = "References".into();
                if self.locations.is_empty() {
                    self.summary = "No file-backed references found".into();
                }
            }
            LanguageEvent::WorkspaceSymbols { query, result }
                if query == self.workspace_symbol_query =>
            {
                self.workspace_symbols_loading = false;
                match result {
                    Ok(symbols) => {
                        self.workspace_symbols = symbols;
                        self.workspace_symbols_error = None;
                    }
                    Err(error) => self.workspace_symbols_error = Some(error),
                }
            }
            LanguageEvent::DocumentSymbols {
                path,
                version,
                result,
            } if self.document_symbols_path.as_ref() == Some(&path)
                && self.current(&path, version)
                && self.document_symbols_version == Some(version) =>
            {
                self.document_symbols_loading = false;
                match result {
                    Ok(symbols) => {
                        self.document_symbols = symbols;
                        self.document_symbols_error = None;
                    }
                    Err(error) => self.document_symbols_error = Some(error),
                }
            }
            LanguageEvent::DocumentEdits {
                result: Err(error),
                path,
                version,
                kind,
            } if self.current(&path, version) => {
                self.summary = format!(
                    "{} failed: {error}",
                    match kind {
                        DocumentEditKind::Format => "Java formatting",
                        DocumentEditKind::OrganizeImports => "Organize imports",
                    }
                );
            }
            _ => {}
        }
    }
    pub fn apply_completion(
        &mut self,
        index: usize,
        document: &mut Document,
    ) -> Result<Option<Vec<(u32, Vec<(usize, usize)>)>>, LspError> {
        let (path, version, position, items) = self
            .completion
            .as_ref()
            .ok_or_else(|| LspError("No completion result".into()))?;
        if path != &document.path
            || document.lsp_position() != *position
            || !self.current(path, *version)
            || self
                .documents
                .get(path)
                .is_none_or(|synced| synced.text != document.text)
        {
            return Err(LspError("The completion is stale; request it again".into()));
        }
        let item = items
            .get(index)
            .cloned()
            .ok_or_else(|| LspError("Completion no longer exists".into()))?;
        if !item.applicable {
            return Err(LspError(
                "This completion requires unsupported snippets or server commands".into(),
            ));
        }
        let range = item
            .range
            .unwrap_or_else(|| document.completion_prefix_range(*position));
        let mut edits = item.additional_edits.clone();
        edits.push((range, item.insert_text.clone()));
        document.apply_lsp_edits(&edits).map_err(LspError)?;
        let tab_stops = if let Some(tab_stops) = item.snippet_tab_stops {
            let insertion_end = document.character_index();
            let insertion_start = insertion_end.saturating_sub(item.insert_text.chars().count());
            Some(
                tab_stops
                    .into_iter()
                    .map(|(index, ranges)| {
                        (
                            index,
                            ranges
                                .into_iter()
                                .map(|(start, end)| {
                                    (insertion_start + start, insertion_start + end)
                                })
                                .collect(),
                        )
                    })
                    .collect(),
            )
        } else {
            None
        };
        self.completion = None;
        self.completion_selection = None;
        Ok(tab_stops)
    }

    pub fn apply_code_action(
        &mut self,
        index: usize,
        document: &mut Document,
    ) -> Result<String, LspError> {
        let (path, version, position, action) = self
            .code_actions
            .as_ref()
            .and_then(|(path, version, position, actions)| {
                actions
                    .get(index)
                    .map(|action| (path, version, position, action))
            })
            .ok_or_else(|| {
                LspError("The selected Java code action is no longer available".into())
            })?;
        if path != &document.path
            || document.lsp_position() != *position
            || !self.is_document_current(path, *version, &document.text)
        {
            self.code_actions = None;
            return Err(LspError(
                "The Java source changed; request code actions again".into(),
            ));
        }
        let title = action.title.clone();
        document.apply_lsp_edits(&action.edits).map_err(LspError)?;
        self.code_actions = None;
        Ok(title)
    }

    pub fn move_completion_selection(&mut self, direction: i32) -> Option<usize> {
        let items = &self.completion.as_ref()?.3;
        let available: Vec<_> = items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| item.applicable.then_some(index))
            .collect();
        if available.is_empty() {
            self.completion_selection = None;
            return None;
        }
        let current = self
            .completion_selection
            .and_then(|selected| available.iter().position(|index| *index == selected))
            .unwrap_or(0);
        let next = if direction < 0 {
            if current == 0 {
                available.len() - 1
            } else {
                current - 1
            }
        } else if current + 1 == available.len() {
            0
        } else {
            current + 1
        };
        let selected = available[next];
        self.completion_selection = Some(selected);
        Some(selected)
    }

    pub fn dismiss_completion(&mut self) {
        self.completion = None;
        self.completion_selection = None;
    }
}

fn maven_root_pom(
    pom_files: &std::collections::HashSet<PathBuf>,
    project_root: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(root_pom) = project_root.map(|root| root.join("pom.xml"))
        && pom_files.contains(&root_pom)
    {
        return Some(root_pom);
    }
    pom_files
        .iter()
        .min_by_key(|path| path.components().count())
        .cloned()
}

fn maven_import_arguments(root_pom_uri: &str) -> Value {
    json!([[root_pom_uri], [], []])
}

/// Compact result views stay in the editor and issue normal editor/app commands.
pub fn render(ui: &mut eframe::egui::Ui, state: &mut crate::app::AppState) {
    use crate::app::events::AppEvent;
    use eframe::egui;
    let stale_completion =
        state
            .language
            .completion
            .as_ref()
            .is_some_and(|(path, version, position, _)| {
                state
                    .active_tab
                    .and_then(|index| state.editor_tabs.get(index))
                    .is_none_or(|tab| {
                        &tab.path != path
                            || tab.document.lsp_position() != *position
                            || !state.language.is_document_current(
                                path,
                                *version,
                                &tab.document.text,
                            )
                    })
            });
    if stale_completion {
        state.language.dismiss_completion();
    }
    let java = state
        .active_tab
        .and_then(|index| state.editor_tabs.get(index))
        .is_some_and(|tab| {
            tab.path
                .extension()
                .is_some_and(|extension| extension == "java")
        });
    if !java {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for (label, command) in [
            ("Complete", EditorCommand::Completion),
            ("Parameter info", EditorCommand::SignatureHelp),
            ("Organize imports", EditorCommand::OrganizeImports),
            ("Hover", EditorCommand::Hover),
            ("Definition", EditorCommand::Definition),
            ("Type definition", EditorCommand::TypeDefinition),
            ("Implementation", EditorCommand::Implementation),
            ("References", EditorCommand::References),
            ("Quick fixes", EditorCommand::CodeActions),
        ] {
            if ui.button(crate::i18n::ui_text(ui, label)).clicked() {
                state.enqueue(AppEvent::EditorCommand(command));
            }
        }
    });
    let mut events = Vec::new();
    ui.small(crate::i18n::message(ui.ctx(), &state.language.summary));
    if state
        .language
        .summary
        .starts_with("Java intelligence unavailable:")
        && !state.language.has_workers()
        && !state.language.has_pending_start()
        && !state.pending_language_restart
        && ui
            .small_button("Retry Java intelligence")
            .on_hover_text(crate::i18n::ui_text(
                ui,
                "Restart JDT LS and retry the connection",
            ))
            .clicked()
    {
        events.push(AppEvent::RestartLanguage);
    }
    if let Some(warning) = &state.language.lombok_warning {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            crate::i18n::message(ui.ctx(), warning),
        );
        if ui
            .small_button("Restart Java intelligence after resolving Lombok")
            .clicked()
        {
            events.push(AppEvent::RestartLanguage);
        }
    }
    let active_path = state
        .active_tab
        .and_then(|index| state.editor_tabs.get(index))
        .map(|tab| tab.path.clone());
    if let Some(path) = &active_path {
        if let Some((error_path, error_version, error)) = &state.language.code_actions_error
            && error_path == path
            && state.language.current(error_path, *error_version)
        {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                crate::i18n::message(ui.ctx(), error),
            );
        }
        if let Some((actions_path, version, position, actions)) = &state.language.code_actions
            && actions_path == path
            && state.language.is_document_current(
                path,
                *version,
                state
                    .active_tab
                    .and_then(|index| state.editor_tabs.get(index))
                    .map(|tab| tab.document.text.as_str())
                    .unwrap_or_default(),
            )
            && state
                .active_tab
                .and_then(|index| state.editor_tabs.get(index))
                .is_some_and(|tab| tab.document.lsp_position() == *position)
        {
            if actions.is_empty() {
                ui.small(crate::i18n::ui_text(
                    ui,
                    "No direct quick fixes at the caret",
                ));
            } else {
                egui::CollapsingHeader::new(format!("Quick fixes ({})", actions.len())).show(
                    ui,
                    |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(120.0)
                            .show(ui, |ui| {
                                for (index, action) in actions.iter().take(30).enumerate() {
                                    if ui.button(&action.title).clicked() {
                                        events.push(AppEvent::ApplyCodeAction(index));
                                    }
                                }
                            });
                    },
                );
            }
        }
        if let Some(items) = state.language.diagnostics.get(path) {
            egui::CollapsingHeader::new(crate::i18n::ui_format(
                ui,
                "Java diagnostics ({count})",
                &[("count", &items.len().to_string())],
            ))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("java_diagnostics")
                    .max_height(100.0)
                    .show(ui, |ui| {
                        for diagnostic in items {
                            let severity = match diagnostic.severity {
                                Some(1) => "Error",
                                Some(2) => "Warning",
                                _ => "Info",
                            };
                            if ui
                                .selectable_label(
                                    false,
                                    format!(
                                        "{} {}:{} {}",
                                        crate::i18n::ui_text(ui, severity),
                                        diagnostic.range.start.line + 1,
                                        diagnostic.range.start.character + 1,
                                        diagnostic.message
                                    ),
                                )
                                .clicked()
                            {
                                events.push(AppEvent::OpenFileAtPosition(
                                    path.clone(),
                                    diagnostic.range.start,
                                ));
                            }
                        }
                    });
            });
        }
        if let Some((hover_path, text)) = &state.language.hover {
            if hover_path == path {
                egui::ScrollArea::vertical()
                    .id_salt("java_hover")
                    .max_height(120.0)
                    .show(ui, |ui| {
                        ui.label(text);
                    });
                if ui
                    .small_button(crate::i18n::ui_text(ui, "Dismiss documentation"))
                    .clicked()
                {
                    state.language.hover = None;
                }
            }
        }
        if let Some((signature_path, _, signatures, active_signature)) =
            &state.language.signature_help
        {
            if signature_path == path {
                if let Some(signature) = signatures.get(*active_signature) {
                    egui::CollapsingHeader::new(crate::i18n::ui_text(ui, "Parameter info"))
                        .default_open(true)
                        .show(ui, |ui| {
                            ui.monospace(&signature.label);
                            if let Some(parameter) = &signature.active_parameter {
                                ui.small(crate::i18n::ui_format(
                                    ui,
                                    "Current parameter: {parameter}",
                                    &[("parameter", parameter)],
                                ));
                            }
                            if !signature.documentation.is_empty() {
                                ui.label(&signature.documentation);
                            }
                        });
                } else {
                    ui.small(crate::i18n::ui_text(
                        ui,
                        "No signature information at this position",
                    ));
                }
            }
        }
    }
    if let Some((path, _, _, items)) = &state.language.completion {
        if active_path.as_ref() == Some(path) {
            ui.label(if items.is_empty() {
                crate::i18n::ui_text(ui, "No completions")
            } else {
                crate::i18n::ui_text(ui, "Completions")
            });
            egui::ScrollArea::vertical()
                .id_salt("java_completions")
                .max_height(140.0)
                .show(ui, |ui| {
                    for (index, item) in items.iter().enumerate() {
                        let selected = state.language.completion_selection == Some(index);
                        let response = ui
                            .add_enabled(
                                item.applicable,
                                egui::Button::new(&item.label).selected(selected),
                            )
                            .on_hover_text(&item.detail);
                        if selected {
                            response.scroll_to_me(Some(egui::Align::Center));
                        }
                        if response.clicked() {
                            events.push(AppEvent::ApplyCompletion(index));
                        }
                    }
                });
            if ui
                .small_button(crate::i18n::ui_text(ui, "Dismiss completions"))
                .clicked()
            {
                state.language.dismiss_completion();
            }
        }
    }
    if !state.language.locations.is_empty() {
        egui::CollapsingHeader::new(format!(
            "{} ({})",
            state.language.locations_title,
            state.language.locations.len()
        ))
        .default_open(true)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("java_locations")
                .max_height(110.0)
                .show(ui, |ui| {
                    for location in &state.language.locations {
                        if ui
                            .selectable_label(
                                false,
                                format!(
                                    "{}:{}:{}",
                                    location.path.display(),
                                    location.range.start.line + 1,
                                    location.range.start.character + 1
                                ),
                            )
                            .clicked()
                        {
                            events.push(AppEvent::OpenFileAtPosition(
                                location.path.clone(),
                                location.range.start,
                            ));
                        }
                    }
                });
        });
        if ui
            .small_button(crate::i18n::ui_text(ui, "Dismiss locations"))
            .clicked()
        {
            state.language.locations.clear();
        }
    }
    for event in events {
        state.enqueue(event);
    }
    ui.separator();
}
