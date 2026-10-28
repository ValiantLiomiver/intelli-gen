use std::path::PathBuf;

use super::ActivePanel;

/// UI intents and service completions cross this boundary. Services should emit
/// events rather than hold mutable references to AppState.
#[derive(Clone, Debug)]
pub enum AppEvent {
    OpenProject(PathBuf),
    NewJavaProjectOpen,
    CreateJavaProject {
        parent: PathBuf,
        name: String,
        package: String,
        kind: crate::project::ProjectKind,
    },
    ProjectOpenCompleted(crate::project::workspace::ProjectOpenEvent),
    ProjectTreeRefreshed(crate::project::workspace::ProjectTreeEvent),
    ProjectTreeChildrenLoaded(crate::project::workspace::ProjectTreeChildrenEvent),
    ProjectTreeRootValidated(crate::project::workspace::ProjectTreeRootValidationEvent),
    ProjectTreeChanged(crate::project::tree_watch::ProjectTreeWatchEvent),
    ValidateProjectTreeRoot {
        request: u64,
    },
    LoadProjectTreeChildren {
        path: PathBuf,
        request: u64,
    },
    RefreshProjectTree(PathBuf),
    RevealInFinder(PathBuf),
    FinderRevealed(Result<(), String>),
    OpenFile(PathBuf),
    OpenFileAtLine(PathBuf, usize),
    OpenFileAtPosition(PathBuf, crate::java::lsp::Position),
    NewJavaClass(PathBuf),
    CreateJavaClass { directory: PathBuf, name: String, package: String },
    QuickOpenOpen,
    SymbolSearchOpen,
    FileStructureOpen,
    GoToLineOpen,
    SearchWorkspaceSymbols(String),
    Find(crate::editor::find_replace::FindAction),
    Search(SearchAction),
    SaveActiveFile,
    SaveAllFiles,
    SaveFile(PathBuf),
    CloseActiveFile,
    CloseFile(PathBuf),
    SaveAndClose(PathBuf),
    DiscardAndClose(PathBuf),
    CancelClose,
    RequestQuit,
    EditorCompleted(crate::editor::EditorCompletion),
    EditorCommand(crate::java::lsp::EditorCommand),
    RenameSymbolOpen,
    RenameSymbolConfirm(String),
    ApplyCompletion(usize),
    ApplyCodeAction(usize),
    Language(crate::java::lsp::LanguageUpdate),
    RestartLanguage,
    ShowPanel(ActivePanel),
    SettingsChanged,
    SettingsSaved {
        generation: u64,
        result: Result<(), String>,
    },
    DiscoverJdks,
    AddJdkPath {
        path: PathBuf,
        target: crate::java::jdk::JdkSelectionTarget,
    },
    JdksDiscovered(crate::java::jdk::JdkDiscoveryEvent),
    Build(BuildAction),
    Maven(MavenAction),
    Gradle(GradleAction),
    GradleTasksCompleted(crate::project::gradle_tasks::GradleTaskEvent),
    MavenViewCompleted {
        root: PathBuf,
        generation: u64,
        result: Result<crate::project::maven_view::MavenView, String>,
        cache_error: Option<String>,
    },
    MavenCacheLoaded {
        root: PathBuf,
        generation: u64,
        result: Result<Option<crate::project::maven_cache::CachedMavenView>, String>,
        current_pom_fingerprint: Result<String, String>,
    },
    MavenPomsChanged(crate::project::maven_watch::MavenPomEvent),
    Run(RunAction),
    RunConfigurationsLoaded(crate::java::run_config::RunConfigurationLoadEvent),
    Terminal(TerminalAction),
    Git(GitAction),
    Debugger(DebuggerAction),
    Status(String),
    Error(String),
}

#[derive(Clone, Debug)]
pub enum BuildAction {
    Build,
    Test,
    RunTest {
        class_name: String,
        module_root: PathBuf,
        test_task: String,
        integration: bool,
    },
    Clean,
    Cancel,
}

#[derive(Clone, Debug)]
pub enum MavenAction {
    ToggleCollapsed,
    Reload {
        root: PathBuf,
    },
    ToggleProfile {
        root: PathBuf,
        profile: String,
    },
    SelectModule {
        root: PathBuf,
        module: Option<PathBuf>,
    },
    RunTask {
        root: PathBuf,
        generation: u64,
        module: Option<PathBuf>,
        goal: String,
    },
}

#[derive(Clone, Debug)]
pub enum GradleAction {
    SelectProject { root: PathBuf, project_path: Option<PathBuf> },
    Reload { root: PathBuf },
    Cancel { root: PathBuf },
    RunTask { root: PathBuf, generation: u64, project_path: Option<PathBuf>, task_path: String },
    ToggleCollapsed,
}

#[derive(Clone, Debug)]
pub enum SearchAction {
    Open,
    Run(crate::search::SearchQuery),
    PreviewReplace(crate::search::SearchQuery, String),
    ApplyReplace,
    Cancel,
}

#[derive(Clone, Debug)]
pub enum TerminalAction {
    Open,
    Close,
    Input(String),
}

#[derive(Clone, Debug)]
pub enum GitAction {
    Refresh,
    Diff(PathBuf),
    Stage(PathBuf),
    Unstage(PathBuf),
    Commit(String),
}

#[derive(Clone, Debug)]
pub enum DebuggerAction {
    Start,
    Stop,
    Continue,
    Pause,
    StepOver,
    StepInto,
    StepOut,
    ToggleBreakpoint {
        source: PathBuf,
        line: u32,
        modified: bool,
    },
    UpdateBreakpoint {
        source: PathBuf,
        line: u32,
        condition: Option<String>,
        enabled: bool,
        modified: bool,
    },
    SelectThread {
        epoch: u64,
        thread: i64,
    },
    SelectFrame {
        epoch: u64,
        frame: i64,
    },
    Inspect {
        epoch: u64,
        selection: u64,
        request: crate::debugger::stack::InspectRequest,
    },
    AddWatch(String),
    RemoveWatch(usize),
}

#[derive(Clone, Debug)]
pub enum RunAction {
    Start,
    Stop,
    Input(String),
    Save(crate::java::run_config::RunConfiguration),
    Delete,
}
