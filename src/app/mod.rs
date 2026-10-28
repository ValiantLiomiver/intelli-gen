pub mod actions;
pub mod events;
mod file_structure;
mod finder;
mod go_to_line;
pub mod layout;
mod quick_open;
mod problems;
mod symbol_search;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use eframe::egui;

use crate::editor::EditorIo;
pub use crate::editor::EditorTab;
use crate::project::maven_view::{MavenView, MavenViewLoader};
use crate::project::{Project, maven_cache::MavenCacheLoader, tree::ProjectTree};
use crate::settings::UserSettings;
use events::AppEvent;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActivePanel {
    #[default]
    Build,
    Run,
    Search,
    Terminal,
    Git,
    Debugger,
    Problems,
}

impl ActivePanel {
    pub const ALL: [Self; 7] = [
        Self::Build,
        Self::Run,
        Self::Search,
        Self::Terminal,
        Self::Git,
        Self::Debugger,
        Self::Problems,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Build => "Build",
            Self::Run => "Run",
            Self::Search => "Search",
            Self::Terminal => "Terminal",
            Self::Git => "Git",
            Self::Debugger => "Debugger",
            Self::Problems => "Problems",
        }
    }
}

pub struct AppState {
    pub project: Option<Project>,
    pub project_open_loader: crate::project::workspace::ProjectOpenLoader,
    pub project_open_generation: u64,
    pub project_opening: bool,
    pub project_tree_loader: crate::project::workspace::ProjectTreeLoader,
    pub project_tree_generation: u64,
    pub project_tree_loading: bool,
    pub search: crate::search::ProjectSearch,
    pub terminal: crate::terminal::TerminalTool,
    pub git: crate::git::GitTool,
    pub build: crate::java::build::BuildTool,
    pub maven_view_loader: MavenViewLoader,
    pub maven_cache_loader: MavenCacheLoader,
    pub maven_pom_watcher: Option<crate::project::maven_watch::MavenPomWatcher>,
    pub retiring_maven_watchers: Vec<crate::project::maven_watch::MavenPomWatcher>,
    pub maven_project_generation: u64,
    pub maven_view: Option<MavenView>,
    pub maven_view_root: Option<PathBuf>,
    pub maven_profiles: HashSet<String>,
    pub maven_profiles_user_modified: bool,
    pub maven_generation: u64,
    pub maven_cache_generation: u64,
    pub maven_loading: bool,
    pub maven_cache_loading: bool,
    pub maven_error: Option<String>,
    pub maven_cache_error: Option<String>,
    pub maven_collapsed: bool,
    pub maven_selected_module: Option<PathBuf>,
    pub gradle_task_loader: crate::project::gradle_tasks::GradleTaskLoader,
    pub gradle_task_root: Option<PathBuf>,
    pub gradle_selected_project: Option<PathBuf>,
    pub gradle_tasks: Vec<crate::project::gradle_tasks::GradleTask>,
    pub gradle_task_filter: String,
    pub gradle_task_generation: u64,
    pub gradle_task_loading: bool,
    pub gradle_task_loaded: bool,
    pub gradle_task_error: Option<String>,
    pub gradle_collapsed: bool,
    pub run: crate::java::run_config::RunTool,
    pub run_configuration_loader: crate::java::run_config::RunConfigurationLoader,
    pub run_configuration_generation: u64,
    pub language: crate::java::lsp::LanguageTool,
    pub debugger: crate::debugger::DebugTool,
    pub pending_language_restart: bool,
    pub pending_navigation:
        std::collections::HashMap<PathBuf, crate::java::lsp::Position>,
    pub project_tree: Option<ProjectTree>,
    pub project_tree_error: Option<String>,
    pub new_java_class: Option<NewJavaClassDialog>,
    pub new_java_project: Option<NewJavaProjectDialog>,
    pub rename_symbol: Option<RenameSymbolDialog>,
    pub pending_rename_request: Option<PendingRenameRequest>,
    pub pending_workspace_edits: HashMap<u64, PendingWorkspaceRename>,
    pub next_workspace_edit_id: u64,
    pub editor_tabs: Vec<EditorTab>,
    pub active_tab: Option<usize>,
    pub editor_io: EditorIo,
    pub loading_files: HashSet<PathBuf>,
    pub pending_close: Option<PathBuf>,
    pub quit_requested: bool,
    pub quit_approved: bool,
    pub active_panel: ActivePanel,
    pub events: Vec<AppEvent>,
    pub settings: UserSettings,
    pub jdk_discovery: crate::java::jdk::JdkDiscovery,
    pub settings_saver: crate::settings::SettingsSaver,
    pub settings_save_generation: u64,
    pub settings_save_pending: bool,
    pub finder_reveal: finder::FinderReveal,
    pub quick_open: quick_open::QuickOpen,
    pub symbol_search: symbol_search::SymbolSearch,
    pub file_structure: file_structure::FileStructure,
    pub go_to_line: go_to_line::GoToLine,
    pub find_replace: crate::editor::find_replace::FindReplace,
    pub jdk_path_input: String,
    pub status: String,
    /// Errors from project, editor and other ordinary actions.
    pub last_error: Option<String>,
    /// Durable startup error; action errors and successful saves cannot clear it.
    pub settings_load_error: Option<String>,
    /// Latest save failure; a successful save clears only this error.
    pub settings_save_error: Option<String>,
    /// Protect unreadable or malformed preferences from being overwritten by defaults.
    pub settings_writable: bool,
}

pub struct NewJavaClassDialog {
    pub directory: PathBuf,
    pub name: String,
    pub package: String,
}

pub struct NewJavaProjectDialog {
    pub parent: PathBuf,
    pub name: String,
    pub package: String,
    pub kind: crate::project::ProjectKind,
}

pub struct RenameSymbolDialog {
    pub path: PathBuf,
    pub revision: u64,
    pub position: crate::java::lsp::Position,
    pub old_name: String,
    pub new_name: String,
}

pub struct PendingWorkspaceRename {
    pub root: PathBuf,
    pub generation: u64,
    pub source_path: PathBuf,
    pub version: i64,
    pub position: crate::java::lsp::Position,
    pub source_revision: u64,
    pub open_revisions: HashMap<PathBuf, u64>,
    pub files: Vec<crate::java::lsp::WorkspaceFileEdits>,
}

pub struct PendingRenameRequest {
    pub root: PathBuf,
    pub generation: u64,
    pub source_path: PathBuf,
    pub source_revision: u64,
    pub position: crate::java::lsp::Position,
    pub open_revisions: HashMap<PathBuf, u64>,
    pub open_versions: HashMap<PathBuf, i64>,
}

impl AppState {
    pub fn enqueue(&mut self, event: AppEvent) {
        self.events.push(event);
    }
}

pub struct IdeaLikeApp {
    pub state: AppState,
}

impl IdeaLikeApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self::new_with_startup_path(cc, None)
    }

    pub fn new_with_startup_path(
        cc: &eframe::CreationContext<'_>,
        startup_path: Option<PathBuf>,
    ) -> Self {
        let (settings, settings_load_error, settings_writable) = match UserSettings::load() {
            Ok(settings) => (settings, None, true),
            Err(error) => (
                UserSettings::default(),
                Some(format!(
                    "{error}. Preferences will not be saved until this is fixed and the app restarted."
                )),
                false,
            ),
        };
        cc.egui_ctx.set_theme(if settings.dark_mode {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        });
        let mut state = AppState {
            project: None,
            project_open_loader: crate::project::workspace::ProjectOpenLoader::default(),
            project_open_generation: 0,
            project_opening: false,
            project_tree_loader: crate::project::workspace::ProjectTreeLoader::default(),
            project_tree_generation: 0,
            project_tree_loading: false,
            search: crate::search::ProjectSearch::new(cc.egui_ctx.clone()),
            terminal: crate::terminal::TerminalTool::default(),
            git: crate::git::GitTool::new(cc.egui_ctx.clone()),
            build: crate::java::build::BuildTool::default(),
            maven_view_loader: MavenViewLoader::default(),
            maven_cache_loader: MavenCacheLoader::default(),
            maven_pom_watcher: None,
            retiring_maven_watchers: Vec::new(),
            maven_project_generation: 0,
            maven_view: None,
            maven_view_root: None,
            maven_profiles: HashSet::new(),
            maven_profiles_user_modified: false,
            maven_generation: 0,
            maven_cache_generation: 0,
            maven_loading: false,
            maven_cache_loading: false,
            maven_error: None,
            maven_cache_error: None,
            maven_collapsed: false,
            maven_selected_module: None,
            gradle_task_loader: crate::project::gradle_tasks::GradleTaskLoader::default(),
            gradle_task_root: None,
            gradle_selected_project: None,
            gradle_tasks: Vec::new(),
            gradle_task_filter: String::new(),
            gradle_task_generation: 0,
            gradle_task_loading: false,
            gradle_task_loaded: false,
            gradle_task_error: None,
            gradle_collapsed: false,
            run: crate::java::run_config::RunTool::default(),
            run_configuration_loader: crate::java::run_config::RunConfigurationLoader::default(),
            run_configuration_generation: 0,
            language: crate::java::lsp::LanguageTool::default(),
            debugger: crate::debugger::DebugTool::default(),
            pending_language_restart: false,
            pending_navigation: Default::default(),
            project_tree: None,
            project_tree_error: None,
            new_java_class: None,
            new_java_project: None,
            rename_symbol: None,
            pending_rename_request: None,
            pending_workspace_edits: HashMap::new(),
            next_workspace_edit_id: 0,
            editor_tabs: Vec::new(),
            active_tab: None,
            editor_io: EditorIo::new(cc.egui_ctx.clone()),
            loading_files: HashSet::new(),
            pending_close: None,
            quit_requested: false,
            quit_approved: false,
            active_panel: ActivePanel::default(),
            events: Vec::new(),
            settings,
            jdk_discovery: crate::java::jdk::JdkDiscovery::default(),
            settings_saver: crate::settings::SettingsSaver::default(),
            settings_save_generation: 0,
            settings_save_pending: false,
            finder_reveal: finder::FinderReveal::default(),
            quick_open: quick_open::QuickOpen::default(),
            symbol_search: symbol_search::SymbolSearch::default(),
            file_structure: file_structure::FileStructure::default(),
                go_to_line: go_to_line::GoToLine::default(),
            find_replace: crate::editor::find_replace::FindReplace::default(),
            jdk_path_input: String::new(),
            status: "Open a Maven or Gradle project to get started".into(),
            last_error: None,
            settings_load_error,
            settings_save_error: None,
            settings_writable,
        };
        if !state.settings.jdks.is_empty() {
            if let Err(error) = state
                .jdk_discovery
                .start_validate(state.settings.jdks.clone())
            {
                state.last_error = Some(error);
            }
        }
        state.language.set_repaint_context(cc.egui_ctx.clone());
        state.language.refresh_tooling(&state.settings);
        if let Some(path) = startup_path {
            state.enqueue(AppEvent::OpenProject(path));
        }
        Self { state }
    }
}

impl eframe::App for IdeaLikeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if ui.ctx().input(|input| input.viewport().close_requested())
            && (!self.state.quit_approved
                || self.state.terminal.has_workers()
                || self.state.build.has_workers()
                || self.state.project_open_loader.has_workers()
                || self.state.project_tree_loader.has_workers()
                || self.state.run_configuration_loader.has_workers()
                || self.state.jdk_discovery.has_worker()
                || self.state.settings_saver.has_workers()
                || self.state.finder_reveal.has_workers()
                || self.state.quick_open.has_workers()
                || self.state.find_replace.has_workers()
                || self.state.maven_view_loader.has_workers()
                || self.state.gradle_task_loader.has_workers()
                || self.state.maven_cache_loader.has_workers()
                || self
                    .state
                    .maven_pom_watcher
                    .as_ref()
                    .is_some_and(crate::project::maven_watch::MavenPomWatcher::has_worker)
                || !self.state.retiring_maven_watchers.is_empty()
                || self.state.language.has_workers()
                || self.state.run.has_workers()
                || self.state.debugger.has_workers()
                || self.state.editor_io.has_workers())
        {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.state.enqueue(AppEvent::RequestQuit);
        }
        for completion in self.state.editor_io.drain() {
            self.state.enqueue(AppEvent::EditorCompleted(completion));
        }
        let active_find_tab = self.state.active_tab.and_then(|index| self.state.editor_tabs.get_mut(index));
        self.state.find_replace.poll(active_find_tab);
        self.state.search.poll();
        self.state.quick_open.poll();
        self.state.terminal.poll();
        self.state.git.poll();
        self.state.build.poll();
        if self.state.build.take_successful_completion() {
            self.state.language.check_lombok_after_successful_build();
        }
        for event in self.state.run_configuration_loader.poll() {
            self.state.enqueue(AppEvent::RunConfigurationsLoaded(event));
        }
        for result in self.state.finder_reveal.poll() {
            self.state.enqueue(AppEvent::FinderRevealed(result));
        }
        for (generation, result) in self.state.settings_saver.poll() {
            self.state
                .enqueue(AppEvent::SettingsSaved { generation, result });
        }
        for event in self.state.project_open_loader.poll() {
            self.state.enqueue(AppEvent::ProjectOpenCompleted(event));
        }
        for event in self.state.project_tree_loader.poll() {
            match event {
                crate::project::workspace::ProjectTreeLoadEvent::Refreshed(event) => {
                    self.state.enqueue(AppEvent::ProjectTreeRefreshed(event));
                }
                crate::project::workspace::ProjectTreeLoadEvent::ChildrenLoaded(event) => {
                    self.state
                        .enqueue(AppEvent::ProjectTreeChildrenLoaded(event));
                }
                crate::project::workspace::ProjectTreeLoadEvent::RootValidated(event) => {
                    self.state
                        .enqueue(AppEvent::ProjectTreeRootValidated(event));
                }
            }
        }
        if let Some(jdks) = self.state.jdk_discovery.poll() {
            self.state.enqueue(AppEvent::JdksDiscovered(jdks));
        }
        for event in self.state.maven_view_loader.poll() {
            self.state.enqueue(AppEvent::MavenViewCompleted {
                root: event.root,
                generation: event.generation,
                result: event.result,
                cache_error: event.cache_error,
            });
        }
        for event in self.state.gradle_task_loader.poll() {
            self.state.enqueue(AppEvent::GradleTasksCompleted(event));
        }
        for event in self.state.maven_cache_loader.poll() {
            self.state.enqueue(AppEvent::MavenCacheLoaded {
                root: event.root,
                generation: event.generation,
                result: event.result,
                current_pom_fingerprint: event.current_pom_fingerprint,
            });
        }
        let pom_events = self
            .state
            .maven_pom_watcher
            .as_mut()
            .map(crate::project::maven_watch::MavenPomWatcher::poll)
            .unwrap_or_default();
        for event in pom_events {
            self.state.enqueue(AppEvent::MavenPomsChanged(event));
        }
        self.state
            .retiring_maven_watchers
            .retain_mut(|watcher| !watcher.reap());
        self.state.run.poll();
        self.state.debugger.poll();
        if let Some((source, line)) = self.state.debugger.inspector.navigation.take() {
            self.state.enqueue(AppEvent::OpenFileAtLine(source, line));
        }
        if self.state.debugger.host_restart_required {
            self.state.debugger.host_restart_required = false;
            self.state.pending_language_restart = true;
        }
        if self.state.pending_language_restart
            && !self.state.debugger.has_workers()
            && !self.state.quit_approved
        {
            actions::restart_language(&mut self.state);
        }
        for event in self.state.language.poll() {
            self.state.enqueue(AppEvent::Language(event));
        }
        if self.state.language.take_lombok_restart_request() {
            self.state.pending_language_restart = true;
        }
        if self.state.terminal.has_workers()
            || self.state.build.has_workers()
            || self.state.project_open_loader.has_workers()
            || self.state.project_tree_loader.has_workers()
            || self.state.run_configuration_loader.has_workers()
            || self.state.jdk_discovery.has_worker()
            || self.state.settings_saver.has_workers()
            || self.state.finder_reveal.has_workers()
            || self.state.quick_open.has_workers()
            || self.state.find_replace.has_workers()
            || self.state.maven_loading
            || self.state.maven_view_loader.has_workers()
            || self.state.gradle_task_loader.has_workers()
            || self.state.maven_cache_loader.has_workers()
            || self
                .state
                .maven_pom_watcher
                .as_ref()
                .is_some_and(crate::project::maven_watch::MavenPomWatcher::has_worker)
            || !self.state.retiring_maven_watchers.is_empty()
            || self.state.language.has_workers()
            || self.state.language.has_pending_start()
            || self.state.run.has_workers()
            || self.state.debugger.has_workers()
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(40));
        }
        actions::keyboard_shortcuts(ui.ctx(), &mut self.state);
        actions::process_events(ui.ctx(), &mut self.state);
        let workbench_enabled = !self.state.quit_approved;
        ui.add_enabled_ui(workbench_enabled, |ui| layout::render(ui, &mut self.state));
        if let Some(delay) = self
            .state
            .project_tree
            .as_ref()
            .and_then(crate::project::tree::ProjectTree::root_validation_delay)
        {
            ui.ctx().request_repaint_after(delay);
        }
        if !self.state.quit_approved {
            self.state.language.sync_documents(&self.state.editor_tabs);
        }
        crate::editor::tabs::close_prompt(ui.ctx(), &mut self.state);
        if self.state.quit_approved {
            self.state.run_configuration_loader.shutdown();
            self.state.terminal.close();
            self.state.search.cancel();
            self.state.quick_open.cancel();
            self.state.build.cancel();
            self.state.maven_view_loader.cancel();
            self.state.gradle_task_loader.cancel();
            self.state.maven_cache_loader.cancel();
            if let Some(watcher) = &mut self.state.maven_pom_watcher {
                watcher.request_cancel();
            }
            for watcher in &mut self.state.retiring_maven_watchers {
                watcher.request_cancel();
            }
            self.state.run.cancel();
            self.state.debugger.cancel();
            // Keep the JDT-owned adapter alive until disconnect/terminateDebuggee
            // completes. Its process group is then killed and reaped by LSP.
            if !self.state.debugger.has_workers() {
                self.state.language.shutdown();
            }
            if self.state.terminal.has_workers()
                || self.state.build.has_workers()
                || self.state.project_open_loader.has_workers()
                || self.state.project_tree_loader.has_workers()
                || self.state.run_configuration_loader.has_workers()
                || self.state.jdk_discovery.has_worker()
                || self.state.settings_saver.has_workers()
                || self.state.finder_reveal.has_workers()
                || self.state.quick_open.has_workers()
                || self.state.find_replace.has_workers()
                || self.state.maven_view_loader.has_workers()
                || self.state.gradle_task_loader.has_workers()
                || self.state.maven_cache_loader.has_workers()
                || self
                    .state
                    .maven_pom_watcher
                    .as_ref()
                    .is_some_and(crate::project::maven_watch::MavenPomWatcher::has_worker)
                || !self.state.retiring_maven_watchers.is_empty()
                || self.state.language.has_workers()
                || self.state.run.has_workers()
                || self.state.debugger.has_workers()
            {
                self.state.status = "Waiting for process cleanup before closing…".into();
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(40));
            } else {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        if !self.state.events.is_empty() {
            ui.ctx().request_repaint();
        }
    }
}
