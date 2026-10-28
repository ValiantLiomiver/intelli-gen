use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::events::{AppEvent, MavenAction};
use super::{AppState, EditorTab};
use crate::editor::EditorCompletion;
use crate::project::{
    ProjectKind,
    maven_view::{MavenModuleView, MavenViewRequest},
};

fn navigate_tab_to_position(tab: &mut EditorTab, position: crate::java::lsp::Position) {
    if position.character == 0 {
        tab.navigate_to_line(position.line as usize + 1);
    } else {
        let character_index = tab
            .document
            .character_index_at_lsp_position(position);
        tab.navigate_to_character_index(character_index);
    }
}

fn open_file_at_position(
    state: &mut AppState,
    path: PathBuf,
    position: crate::java::lsp::Position,
) {
    if let Some(index) = state.editor_tabs.iter().position(|tab| tab.path == path) {
        state.active_tab = Some(index);
        navigate_tab_to_position(&mut state.editor_tabs[index], position);
    } else {
        state.pending_navigation.insert(path.clone(), position);
        state.enqueue(AppEvent::OpenFile(path));
    }
}

pub fn open_project_dialog(state: &mut AppState) {
    if state.quit_approved {
        return;
    }
    let mut dialog = rfd::FileDialog::new().set_title("Open Java project");
    if let Some(project) = &state.project {
        dialog = dialog.set_directory(&project.root);
    }
    if let Some(path) = dialog.pick_folder() {
        state.enqueue(AppEvent::OpenProject(path));
    }
}

pub fn open_file_dialog(state: &mut AppState) {
    if state.quit_approved {
        return;
    }
    let mut dialog = rfd::FileDialog::new().set_title("Open file");
    if let Some(project) = &state.project {
        dialog = dialog.set_directory(&project.root);
    }
    if let Some(path) = dialog.pick_file() {
        state.enqueue(AppEvent::OpenFile(path));
    }
}

pub fn keyboard_shortcuts(ctx: &egui::Context, state: &mut AppState) {
    if state.quit_approved {
        return;
    }
    if ctx.input_mut(|input| input.consume_key(egui::Modifiers::SHIFT, egui::Key::F6)) {
        state.enqueue(AppEvent::RenameSymbolOpen);
    }
    if ctx.input_mut(|input| input.consume_key(egui::Modifiers::ALT, egui::Key::Enter)) {
        state.enqueue(AppEvent::EditorCommand(crate::java::lsp::EditorCommand::CodeActions));
    }
    let comment_editor = state.active_tab.and_then(|index| state.editor_tabs.get(index))
        .filter(|tab| tab.path.extension().is_some_and(|extension| extension == "java"))
        .map(|tab| if tab.document.text.len() > crate::editor::document::PAGED_EDITOR_BYTES {
            egui::Id::new(("paged_source_editor", &tab.path))
        } else {
            egui::Id::new(("source_editor", &tab.path))
        });
    if comment_editor.is_some_and(|id| ctx.memory(|memory| memory.focused() == Some(id)))
        && ctx.input_mut(|input| {
            input.consume_key(egui::Modifiers::COMMAND, egui::Key::Slash)
                || input.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Slash)
        })
    {
        state.enqueue(AppEvent::ToggleJavaLineComment);
    }
    let completion_editor = state
        .active_tab
        .and_then(|index| state.editor_tabs.get(index))
        .and_then(|tab| {
            let is_current = state.language.completion.as_ref().is_some_and(
                |(path, version, position, items)| {
                    path == &tab.path
                        && *position == tab.document.lsp_position()
                        && state.language.is_document_current(path, *version, &tab.document.text)
                        && items.iter().any(|item| item.applicable)
                },
            );
            is_current.then(|| {
                if tab.document.text.len() > crate::editor::document::PAGED_EDITOR_BYTES {
                    egui::Id::new(("paged_source_editor", &tab.path))
                } else {
                    egui::Id::new(("source_editor", &tab.path))
                }
            })
        });
    if completion_editor.is_some_and(|id| ctx.memory(|memory| memory.focused() == Some(id))) {
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
            state.language.move_completion_selection(1);
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
            state.language.move_completion_selection(-1);
        }
        if let Some(index) = state.language.completion_selection {
            let accept = ctx.input_mut(|input| {
                input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                    || input.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
            });
            if accept {
                state.enqueue(AppEvent::ApplyCompletion(index));
            }
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            state.language.dismiss_completion();
        }
    }
    let snippet_editor = state
        .active_tab
        .and_then(|index| state.editor_tabs.get(index))
        .filter(|tab| tab.has_snippet_tab_stops())
        .map(|tab| {
            if tab.document.text.len() > crate::editor::document::PAGED_EDITOR_BYTES {
                egui::Id::new(("paged_source_editor", &tab.path))
            } else {
                egui::Id::new(("source_editor", &tab.path))
            }
        });
    let snippet_direction = if snippet_editor.is_some_and(|id| ctx.memory(|memory| memory.focused() == Some(id))) {
        ctx.input_mut(|input| {
            if input.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab) {
                Some(true)
            } else if input.consume_key(egui::Modifiers::NONE, egui::Key::Tab) {
                Some(false)
            } else {
                None
            }
        })
    } else {
        None
    };
    if let Some(backward) = snippet_direction {
        if let Some(tab) = state.active_tab.and_then(|index| state.editor_tabs.get_mut(index)) {
            tab.advance_snippet_tab_stop(backward);
        }
    }
    let indent_target = state.active_tab.and_then(|index| state.editor_tabs.get(index))
        .filter(|tab| tab.path.extension().is_some_and(|extension| extension == "java"))
        .filter(|_| snippet_direction.is_none())
        .map(|tab| if tab.document.text.len() > crate::editor::document::PAGED_EDITOR_BYTES {
                egui::Id::new(("paged_source_editor", &tab.path))
            } else {
                egui::Id::new(("source_editor", &tab.path))
            });
    if indent_target.is_some_and(|id| ctx.memory(|memory| memory.focused() == Some(id))) {
        let outdent = ctx.input_mut(|input| {
            if input.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab) {
                Some(true)
            } else if input.consume_key(egui::Modifiers::NONE, egui::Key::Tab) {
                Some(false)
            } else {
                None
            }
        });
        if let Some(outdent) = outdent {
            state.enqueue(AppEvent::IndentJava { outdent });
        }
    }
    if ctx.input_mut(|input| {
        input.consume_key(
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            egui::Key::S,
        )
    }) {
        state.enqueue(AppEvent::SaveAllFiles);
    } else if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::S)) {
        state.enqueue(AppEvent::SaveActiveFile);
    }
    if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::W)) {
        state.enqueue(AppEvent::CloseActiveFile);
    }
    if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::P)) {
        state.enqueue(AppEvent::QuickOpenOpen);
    }
    if ctx.input_mut(|input| {
        input.consume_key(
            egui::Modifiers::CTRL | egui::Modifiers::ALT,
            egui::Key::O,
        )
    }) {
        state.enqueue(AppEvent::EditorCommand(
            crate::java::lsp::EditorCommand::OrganizeImports,
        ));
    }
    if ctx.input_mut(|input| {
        input.consume_key(
            egui::Modifiers::COMMAND | egui::Modifiers::ALT,
            egui::Key::O,
        )
    }) {
        state.enqueue(AppEvent::SymbolSearchOpen);
    }
    if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::F12)) {
        state.enqueue(AppEvent::FileStructureOpen);
    }
    if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::L)) {
        state.enqueue(AppEvent::GoToLineOpen);
    }
    if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
        state.enqueue(AppEvent::Find(
            crate::editor::find_replace::FindAction::Open { replace: false },
        ));
    }
    if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::R)) {
        state.enqueue(AppEvent::Find(
            crate::editor::find_replace::FindAction::Open { replace: true },
        ));
    }
    if ctx.input_mut(|input| {
        input.consume_key(
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            egui::Key::F,
        )
    }) {
        state.enqueue(AppEvent::Search(super::events::SearchAction::Open));
    }
    if ctx.input_mut(|input| {
        input.consume_key(
            egui::Modifiers::COMMAND | egui::Modifiers::ALT,
            egui::Key::L,
        )
    }) {
        state.enqueue(AppEvent::EditorCommand(
            crate::java::lsp::EditorCommand::Format,
        ));
    }
    if ctx.input_mut(|input| {
        input.consume_key(
            egui::Modifiers::COMMAND | egui::Modifiers::ALT,
            egui::Key::B,
        )
    }) {
        state.enqueue(AppEvent::EditorCommand(
            crate::java::lsp::EditorCommand::Implementation,
        ));
    }
    if ctx.input_mut(|input| {
        input.consume_key(
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            egui::Key::B,
        )
    }) {
        state.enqueue(AppEvent::EditorCommand(
            crate::java::lsp::EditorCommand::TypeDefinition,
        ));
    }
    for (modifiers, key, command) in [
        (
            egui::Modifiers::CTRL,
            egui::Key::Space,
            crate::java::lsp::EditorCommand::Completion,
        ),
        (
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            egui::Key::Space,
            crate::java::lsp::EditorCommand::SignatureHelp,
        ),
        (
            egui::Modifiers::NONE,
            egui::Key::F1,
            crate::java::lsp::EditorCommand::Hover,
        ),
        (
            egui::Modifiers::SHIFT,
            egui::Key::F12,
            crate::java::lsp::EditorCommand::References,
        ),
        (
            egui::Modifiers::NONE,
            egui::Key::F12,
            crate::java::lsp::EditorCommand::Definition,
        ),
    ] {
        if ctx.input_mut(|input| input.consume_key(modifiers, key)) {
            state.enqueue(AppEvent::EditorCommand(command));
        }
    }
    let open_project = ctx.input_mut(|input| {
        input.consume_key(
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            egui::Key::O,
        )
    });
    let open_file =
        ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::O));
    if open_project {
        open_project_dialog(state);
    } else if open_file {
        open_file_dialog(state);
    }
}

/// The sole shell reducer. Later tasks dispatch service commands here and return
/// completions as AppEvents; no file contents are read in the rendering pass.
pub fn process_events(context: &egui::Context, state: &mut AppState) {
    for event in std::mem::take(&mut state.events) {
        // Approval is based on the current buffers and completed disk writes.
        // Cleanup can span frames; queued/new UI intents cannot mutate that
        // approved snapshot or start more work while the viewport stays open.
        if state.quit_approved
            && !matches!(
                &event,
                AppEvent::EditorCompleted(_)
                    | AppEvent::Language(_)
                    | AppEvent::RequestQuit
                    | AppEvent::Status(_)
                    | AppEvent::Error(_)
                    | AppEvent::SettingsSaved { .. }
                    | AppEvent::FinderRevealed(_)
            )
        {
            continue;
        }
        match event {
            AppEvent::OpenProject(path) => {
                state.pending_run = None;
                state.quick_open.cancel();
                state.symbol_search.close();
                state.new_java_class = None;
                state.new_java_project = None;
                state.rename_symbol = None;
                state.pending_rename_request = None;
                state.pending_workspace_edits.clear();
                state.pending_workspace_replacements.clear();
                state.project_open_generation = state.project_open_generation.wrapping_add(1);
                state.project_opening = true;
                state.status = format!("Opening {}…", path.display());
                if let Err(error) = state
                    .project_open_loader
                    .start(path, state.project_open_generation)
                {
                    state.project_opening = false;
                    state.last_error = Some(error);
                }
            }
            AppEvent::ProjectOpenCompleted(event) => {
                if event.generation != state.project_open_generation {
                    continue;
                }
                state.project_opening = false;
                let opened = match event.result {
                    Ok(opened) => opened,
                    Err(error) => {
                        state.last_error = Some(error);
                        state.status = "Project open failed".into();
                        continue;
                    }
                };
                let startup_file = opened.open_file;
                let root = opened.root;
                let project = opened.project;
                state.quick_open.set_project(Some(root.clone()));
                if state.project.as_ref().map(|project| &project.root) != Some(&root) {
                    state.build.reset();
                }
                if state.pending_run.as_ref().is_some_and(|pending| pending.root != root) {
                    state.pending_run = None;
                }
                state.run.begin_project_load();
                state.run_configuration_generation =
                    state.run_configuration_generation.wrapping_add(1);
                if let Err(error) = state
                    .run_configuration_loader
                    .start(root.clone(), state.run_configuration_generation)
                {
                    state.run.finish_project_load(Err(error));
                }
                state.debugger.open_project(root.clone());
                state.search.reset();
                state.terminal.close();
                state.git.reset();
                state
                    .git
                    .dispatch(root.clone(), super::events::GitAction::Refresh);
                state.last_error = project.discovery_error.clone();
                state.status = if let Some(model) = &project.model {
                    if model.kind == crate::project::ProjectKind::Unknown {
                        format!(
                            "Opened {} — no Maven or Gradle project found",
                            root.display()
                        )
                    } else {
                        format!(
                            "Opened {} — {} build at {} ({} modules)",
                            root.display(),
                            model.kind,
                            model.root.display(),
                            model.modules.len()
                        )
                    }
                } else {
                    format!("Opened {} — build discovery failed", root.display())
                };
                state.maven_view_loader.cancel();
                state.maven_cache_loader.cancel();
                state.gradle_task_loader.cancel();
                retire_maven_watcher(state);
                retire_project_tree_watcher(state);
                state.project_tree_watch_generation = state.project_tree_watch_generation.wrapping_add(1);
                state.project_tree_generation = state.project_tree_generation.wrapping_add(1);
                state.maven_project_generation = state.maven_project_generation.wrapping_add(1);
                state.project_tree_loading = false;
                state.project = Some(project);
                if let Some(path) = startup_file {
                    state.enqueue(AppEvent::OpenFile(path));
                }
                state.maven_generation = state.maven_generation.wrapping_add(1);
                state.maven_cache_generation = state.maven_cache_generation.wrapping_add(1);
                state.maven_view = None;
                state.maven_view_root = state
                    .project
                    .as_ref()
                    .and_then(|project| project.model.as_ref())
                    .filter(|model| model.kind == ProjectKind::Maven)
                    .map(|model| model.root.clone());
                state.maven_profiles.clear();
                state.maven_profiles_user_modified = false;
                state.maven_selected_module = None;
                state.maven_loading = false;
                state.maven_cache_loading = false;
                state.maven_error = None;
                state.maven_cache_error = None;
                state.gradle_task_generation = state.gradle_task_generation.wrapping_add(1);
                state.gradle_task_root = state
                    .project
                    .as_ref()
                    .and_then(|project| project.model.as_ref())
                    .filter(|model| model.kind == ProjectKind::Gradle)
                    .map(|model| model.root.clone());
                state.gradle_selected_project = None;
                state.gradle_tasks.clear();
                state.gradle_task_filter.clear();
                state.gradle_task_loading = false;
                state.gradle_task_loaded = false;
                state.gradle_task_error = None;
                if let Some(cache_root) = state.maven_view_root.clone() {
                    state.maven_cache_loading = true;
                    if let Err(error) = state
                        .maven_cache_loader
                        .start(cache_root, state.maven_cache_generation)
                    {
                        state.maven_cache_loading = false;
                        state.maven_cache_error = Some(error);
                    }
                }
                if let Some(model) = state
                    .project
                    .as_ref()
                    .and_then(|project| project.model.as_ref())
                    .filter(|model| model.kind == ProjectKind::Maven)
                    .cloned()
                {
                    match crate::project::maven_watch::MavenPomWatcher::start(
                        model.root.clone(),
                        crate::project::maven_watch::project_pom_paths(&model),
                        state.maven_project_generation,
                    ) {
                        Ok(watcher) => state.maven_pom_watcher = Some(watcher),
                        Err(error) => state.maven_error = Some(error),
                    }
                }
                restart_language(state);
                match opened.tree {
                    Ok(tree) => {
                        state.project_tree = Some(tree);
                        state.project_tree_error = None;
                    }
                    Err(error) => {
                        state.project_tree = None;
                        state.project_tree_error = Some(error);
                    }
                }
                if let Some(project) = &state.project {
                    match crate::project::tree_watch::ProjectTreeWatcher::start(
                        project.root.clone(), state.project_tree_watch_generation,
                    ) {
                        Ok(watcher) => state.project_tree_watcher = Some(watcher),
                        Err(error) => state.project_tree_error = Some(error),
                    }
                }
                state.settings.remember_project(root);
                save_settings(state);
            }
            AppEvent::RefreshProjectTree(path) => {
                let Some(root) = state.project.as_ref().map(|project| project.root.clone()) else {
                    continue;
                };
                if path == root {
                    if state.project_tree.is_none() {
                        state.project_tree_generation = state.project_tree_generation.wrapping_add(1);
                        state.project_tree_loading = true;
                        if let Err(error) = state.project_tree_loader.start(root, state.project_tree_generation) {
                            state.project_tree_loading = false;
                            state.project_tree_error = Some(error);
                        }
                        continue;
                    }
                    let request = state.project_tree.as_mut().and_then(|tree| tree.begin_refresh(&root));
                    if let Some(request) = request {
                        state.project_tree_loading = true;
                        if let Err(error) = state.project_tree_loader.start_children(root.clone(), root.clone(), request) {
                            state.project_tree_loading = false;
                            if let Some(tree) = &mut state.project_tree {
                                tree.apply_children(&root, request, Err(error.clone()));
                            }
                            state.project_tree_error = Some(error);
                        }
                    }
                } else if let Some(request) = state
                    .project_tree
                    .as_mut()
                    .and_then(|tree| tree.begin_refresh(&path))
                {
                    if let Err(error) = state.project_tree_loader.start_children(
                        root,
                        path.clone(),
                        request,
                    ) && let Some(tree) = &mut state.project_tree
                    {
                        tree.apply_children(&path, request, Err(error));
                    }
                }
            }
            AppEvent::ProjectTreeRefreshed(event) => {
                if event.generation != state.project_tree_generation
                    || state.project.as_ref().is_none_or(|project| project.root != event.root)
                { continue; }
                state.project_tree_loading = false;
                match event.result {
                    Ok(tree) => {
                        state.project_tree = Some(tree);
                        state.project_tree_error = None;
                    }
                    Err(error) => state.project_tree_error = Some(error),
                }
            }
            AppEvent::ProjectTreeChanged(event) => {
                if event.generation != state.project_tree_watch_generation
                    || state.project.as_ref().is_none_or(|project| project.root != event.root)
                { continue; }
                if let Some(error) = event.error {
                    state.project_tree_error = Some(error);
                    continue;
                }
                if state.project_tree.is_none() {
                    state.project_tree_generation = state.project_tree_generation.wrapping_add(1);
                    state.project_tree_loading = true;
                    if let Err(error) = state.project_tree_loader.start(event.root, state.project_tree_generation) {
                        state.project_tree_loading = false;
                        state.project_tree_error = Some(error);
                    }
                    continue;
                }
                let Some(tree) = state.project_tree.as_mut() else { continue; };
                let targets = if event.rescan || event.paths.is_empty() {
                    tree.begin_refresh_all_expanded()
                } else {
                    tree.begin_watched_refreshes(&event.paths)
                };
                if targets.iter().any(|(parent, _)| parent == &event.root) {
                    state.project_tree_loading = true;
                }
                for (parent, request) in targets {
                    if let Err(error) = state.project_tree_loader.start_children(event.root.clone(), parent.clone(), request)
                        && let Some(tree) = &mut state.project_tree
                    {
                        tree.apply_children(&parent, request, Err(error));
                        if parent == event.root { state.project_tree_loading = false; }
                    }
                }
            }
            AppEvent::LoadProjectTreeChildren { path, request } => {
                if let Some(root) = state.project.as_ref().map(|project| project.root.clone()) {
                    if let Err(error) =
                        state
                            .project_tree_loader
                            .start_children(root, path.clone(), request)
                    {
                        if let Some(tree) = &mut state.project_tree {
                            tree.apply_children(&path, request, Err(error));
                        }
                    }
                }
            }
            AppEvent::ProjectTreeRootValidated(event) => {
                if state
                    .project
                    .as_ref()
                    .is_some_and(|project| project.root == event.root)
                {
                    if let Some(tree) = &mut state.project_tree {
                        tree.apply_root_validation(event.request, event.result);
                    }
                }
            }
            AppEvent::ProjectTreeChildrenLoaded(event) => {
                if state
                    .project
                    .as_ref()
                    .is_some_and(|project| project.root == event.root)
                {
                    if event.parent == event.root {
                        state.project_tree_loading = false;
                        state.project_tree_error = event.result.as_ref().err().cloned();
                    }
                    if let Some(tree) = &mut state.project_tree {
                        tree.apply_children(&event.parent, event.request, event.result);
                    }
                }
            }
            AppEvent::ValidateProjectTreeRoot { request } => {
                if let Some(root) = state.project.as_ref().map(|project| project.root.clone()) {
                    if let Err(error) = state
                        .project_tree_loader
                        .start_root_validation(root, request)
                    {
                        if let Some(tree) = &mut state.project_tree {
                            tree.apply_root_validation(request, Err(error));
                        }
                    }
                }
            }
            AppEvent::RevealInFinder(path) => {
                if let Err(error) = state.finder_reveal.start(path) {
                    state.last_error = Some(error);
                }
            }
            AppEvent::FinderRevealed(result) => {
                if let Err(error) = result {
                    state.last_error = Some(error);
                }
            }
            AppEvent::OpenFile(path) => {
                if let Some(index) = state.editor_tabs.iter().position(|tab| tab.path == path) {
                    state.active_tab = Some(index);
                } else if !state.quit_requested && state.loading_files.insert(path.clone()) {
                    match state.editor_io.load(path.clone()) {
                        Ok(()) => state.status = format!("Opening {}…", path.display()),
                        Err(error) => {
                            state.loading_files.remove(&path);
                            state.last_error = Some(error);
                        }
                    }
                }
            }
            AppEvent::NewJavaClass(directory) => {
                if !state.quit_requested && state.project.is_some() {
                    state.new_java_class = Some(super::NewJavaClassDialog {
                        directory,
                        name: String::new(),
                        package: String::new(),
                    });
                }
            }
            AppEvent::NewJavaProjectOpen => {
                if !state.quit_requested {
                    let parent = state.project.as_ref()
                        .and_then(|project| project.root.parent().map(Path::to_path_buf))
                        .or_else(|| directories::UserDirs::new().map(|user_dirs| user_dirs.home_dir().to_path_buf()))
                        .unwrap_or_else(|| PathBuf::from("."));
                    state.new_java_project = Some(super::NewJavaProjectDialog {
                        parent,
                        name: "JavaProject".into(),
                        package: "com.example".into(),
                        kind: ProjectKind::Maven,
                    });
                }
            }
            AppEvent::CreateJavaProject { parent, name, package, kind } => {
                if state.quit_requested { continue; }
                state.project_open_generation = state.project_open_generation.wrapping_add(1);
                state.project_opening = true;
                state.status = format!("Creating {kind} project {name}…");
                let request = crate::project::workspace::NewProjectRequest { parent, name, package, kind };
                if let Err(error) = state.project_open_loader.start_new(request, state.project_open_generation) {
                    state.project_opening = false;
                    state.last_error = Some(error);
                }
            }
            AppEvent::CreateJavaClass { directory, name, package } => {
                let Some(root) = state.project.as_ref().map(|project| project.root.clone()) else { continue; };
                match state.editor_io.create_java_class(root, directory, name, package) {
                    Ok(()) => state.status = "Creating Java class…".into(),
                    Err(error) => state.last_error = Some(error),
                }
            }
            AppEvent::QuickOpenOpen => {
                if !state.quit_requested {
                    state.quick_open.open();
                }
            }
            AppEvent::SymbolSearchOpen => {
                if !state.quit_requested {
                    state.symbol_search.open();
                }
            }
            AppEvent::FileStructureOpen => {
                if !state.quit_requested {
                    state.language.sync_documents(&state.editor_tabs);
                    if let Some(tab) = state
                        .active_tab
                        .and_then(|index| state.editor_tabs.get(index))
                    {
                        if tab
                            .path
                            .extension()
                            .is_some_and(|extension| extension == "java")
                        {
                            state.file_structure.open(tab.path.clone());
                            state.language.request_document_symbols(&tab.document);
                        } else {
                            state.status =
                                "File Structure is available for Java source files".into();
                        }
                    } else {
                        state.status = "Open a Java file to view its structure".into();
                    }
                }
            }
            AppEvent::GoToLineOpen => {
                if !state.quit_requested {
                    if state.active_tab.is_some() {
                        state.go_to_line.open();
                    } else {
                        state.status = "Open a file before navigating to a line".into();
                    }
                }
            }
            AppEvent::SearchWorkspaceSymbols(query) => {
                if !state.quit_requested {
                    state.language.search_workspace_symbols(query);
                }
            }
            AppEvent::Find(action) => {
                let tab = state
                    .active_tab
                    .and_then(|index| state.editor_tabs.get_mut(index));
                state.find_replace.apply(action, tab);
            }
            AppEvent::OpenFileAtLine(path, line) => {
                open_file_at_position(
                    state,
                    path,
                    crate::java::lsp::Position {
                        line: line.saturating_sub(1).min(u32::MAX as usize) as u32,
                        character: 0,
                    },
                );
            }
            AppEvent::OpenFileAtPosition(path, position) => {
                open_file_at_position(state, path, position);
            }
            AppEvent::Search(action) => match action {
                super::events::SearchAction::Open => {
                    state.active_panel = super::ActivePanel::Search;
                    let show_window_changed = !state.settings.show_tool_window;
                    state.settings.show_tool_window = true;
                    state.search.focus_requested = true;
                    if show_window_changed {
                        save_settings(state);
                    }
                }
                super::events::SearchAction::Run(query) => {
                    if let Some(project) = &state.project {
                        state.search.start(project.root.clone(), query);
                    }
                }
                super::events::SearchAction::PreviewReplace(query, replacement) => {
                    if let Some(project) = &state.project {
                        let project_root = project.root.clone();
                        let relevant_tabs: Vec<_> = state.editor_tabs.iter()
                            .filter(|tab| tab.path.starts_with(&project_root))
                            .collect();
                        let dirty_tabs: Vec<_> = relevant_tabs.iter()
                            .filter(|tab| tab.document.is_dirty())
                            .collect();
                        let dirty_bytes = dirty_tabs.iter().try_fold(0usize, |total, tab| total.checked_add(tab.document.text.len()));
                        if dirty_tabs.len() > 200 || dirty_bytes.is_none_or(|bytes| bytes > crate::editor::MAX_WORKSPACE_EDIT_BYTES) {
                            state.search.message = "Open unsaved files exceed the project replacement limit".into();
                            return;
                        }
                        let open_documents = relevant_tabs.into_iter().map(|tab| crate::search::OpenDocumentSnapshot {
                            path: tab.document.path.clone(),
                            revision: tab.document.revision,
                            text: tab.document.is_dirty().then(|| tab.document.text.clone()),
                        }).collect();
                        state.search.start_replace_preview(project_root, query, replacement, open_documents);
                    }
                }
                super::events::SearchAction::ApplyReplace => apply_project_replace(state),
                super::events::SearchAction::Cancel => state.search.cancel(),
            },
            AppEvent::Terminal(action) => match action {
                super::events::TerminalAction::Open => {
                    if let Some(project) = &state.project {
                        state.terminal.open(project.root.clone(), context.clone());
                    }
                }
                super::events::TerminalAction::Close => state.terminal.close(),
                super::events::TerminalAction::Input(input) => {
                    state.terminal.send(input.into_bytes())
                }
            },
            AppEvent::Git(action) => {
                if let Some(project) = &state.project {
                    state.git.dispatch(project.root.clone(), action);
                }
            }
            AppEvent::SaveActiveFile => {
                if let Some(path) = active_path(state) {
                    save_file(state, &path);
                }
            }
            AppEvent::SaveAllFiles => {
                let paths: Vec<_> = state
                    .editor_tabs
                    .iter()
                    // The pending snapshot can differ even after edits return to
                    // the old baseline; queue another save for those tabs too.
                    .filter(|tab| tab.document.is_dirty() || tab.saving || tab.pending_rename_to.is_some())
                    .map(|tab| tab.path.clone())
                    .collect();
                for path in paths {
                    save_file(state, &path);
                }
            }
            AppEvent::SaveFile(path) => { save_file(state, &path); }
            AppEvent::CloseActiveFile => {
                if let Some(path) = active_path(state) {
                    close_file(state, &path);
                }
            }
            AppEvent::CloseFile(path) => close_file(state, &path),
            AppEvent::SaveAndClose(path) => {
                if let Some(tab) = state.editor_tabs.iter_mut().find(|tab| tab.path == path) {
                    tab.close_after_save = true;
                    save_file(state, &path);
                }
            }
            AppEvent::DiscardAndClose(path) => {
                // A pending write must finish before a document can be discarded.
                if state
                    .editor_tabs
                    .iter()
                    .any(|tab| tab.path == path && !tab.saving)
                {
                    remove_tab(state, &path);
                }
            }
            AppEvent::CancelClose => {
                state.pending_close = None;
                state.quit_requested = false;
            }
            AppEvent::RequestQuit => state.quit_requested = true,
            AppEvent::EditorCompleted(completion) => editor_completed(state, completion),
            AppEvent::EditorCommand(command) => {
                let tab_size = state.settings.java_indent_size.clamp(1, 8);
                let insert_spaces = state.settings.java_insert_spaces;
                state.language.sync_documents(&state.editor_tabs);
                if let Some(tab) = state
                    .active_tab
                    .and_then(|index| state.editor_tabs.get(index))
                {
                    state.language.request(command, &tab.document, tab_size, insert_spaces);
                }
            }
            AppEvent::RenameSymbolOpen => begin_symbol_rename(state),
            AppEvent::RenameSymbolConfirm(new_name) => {
                if state.pending_rename_request.is_some() || !state.pending_workspace_edits.is_empty() {
                    state.last_error = Some("Wait for the current Java rename to finish".into());
                    continue;
                }
                let Some(dialog) = state.rename_symbol.take() else { continue; };
                let Some(document) = state.editor_tabs.iter().find(|tab| tab.path == dialog.path).map(|tab| tab.document.clone()) else {
                    state.last_error = Some("The Java file closed before rename started".into());
                    continue;
                };
                if document.revision != dialog.revision || document.lsp_position() != dialog.position {
                    state.last_error = Some("The Java source changed; invoke Rename Symbol again".into());
                    continue;
                }
                let Some(root) = state.project.as_ref().map(|project| project.root.clone()) else {
                    state.last_error = Some("Cannot resolve the open project root".into());
                    continue;
                };
                let open_revisions = state.editor_tabs.iter().map(|tab| (tab.path.clone(), tab.document.revision)).collect();
                let generation = state.language.generation();
                state.language.sync_documents(&state.editor_tabs);
                let synchronized_versions = state.language.document_versions();
                let open_java_documents_synced = state.editor_tabs.iter()
                    .filter(|tab| tab.path.extension().is_some_and(|extension| extension == "java")
                        && tab.path.starts_with(&root))
                    .all(|tab| synchronized_versions.get(&tab.path).is_some_and(|version|
                        state.language.is_document_current(&tab.path, *version, &tab.document.text)));
                if !open_java_documents_synced {
                    state.last_error = Some("Wait for open Java files to synchronize before renaming".into());
                    continue;
                }
                let open_versions = synchronized_versions;
                match state.language.request_rename(&document, dialog.position, new_name) {
                    Ok(()) => state.pending_rename_request = Some(super::PendingRenameRequest {
                        root, generation, source_path: dialog.path, source_revision: dialog.revision,
                        position: dialog.position, open_revisions, open_versions,
                    }),
                    Err(error) => state.last_error = Some(error.to_string()),
                }
            }
            AppEvent::ApplyCompletion(index) => {
                if let Some(tab) = state
                    .active_tab
                    .and_then(|index| state.editor_tabs.get_mut(index))
                {
                    match state.language.apply_completion(index, &mut tab.document) {
                        Ok(Some(stops)) if !stops.is_empty() => tab.set_snippet_tab_stops(stops),
                        Ok(Some(_)) | Ok(None) => tab.restore_document_cursor(),
                        Err(error) => state.language.summary = error.to_string(),
                    }
                }
            }
            AppEvent::ToggleJavaLineComment => {
                if let Some(tab) = state.active_tab.and_then(|index| state.editor_tabs.get_mut(index)) {
                    tab.toggle_java_line_comment(context);
                }
            }
            AppEvent::IndentJava { outdent } => {
                let indent_size = state.settings.java_indent_size.clamp(1, 8);
                let insert_spaces = state.settings.java_insert_spaces;
                if let Some(tab) = state.active_tab.and_then(|index| state.editor_tabs.get_mut(index)) {
                    tab.indent_java_selection(context, outdent, indent_size, insert_spaces);
                }
            }
            AppEvent::ApplyCodeAction(index) => {
                if let Some(tab) = state.active_tab.and_then(|tab_index| state.editor_tabs.get_mut(tab_index)) {
                    match state.language.apply_code_action(index, &mut tab.document) {
                        Ok(title) => {
                            tab.restore_document_cursor();
                            state.language.summary = format!("Applied Java code action: {title}");
                        }
                        Err(error) => state.language.summary = error.to_string(),
                    }
                }
            }
            AppEvent::Language(update) => {
                let generation = update.generation;
                let document_edits = match &update.event {
                    crate::java::lsp::LanguageEvent::DocumentEdits { path, version, kind, result } => {
                        Some((path.clone(), *version, *kind, result.clone()))
                    }
                    _ => None,
                };
                let rename_edits = match &update.event {
                    crate::java::lsp::LanguageEvent::RenameEdits { path, version, position, result } => {
                        Some((path.clone(), *version, *position, result.clone()))
                    }
                    _ => None,
                };
                let accepted = state.language.accepts_generation(generation);
                state.language.receive(update);
                if accepted {
                    if let Some((path, version, position, result)) = rename_edits {
                        let Some(snapshot) = state.pending_rename_request.take() else {
                            state.language.summary = "Discarded Java rename without a current request".into();
                            continue;
                        };
                        if snapshot.source_path != path || snapshot.position != position || snapshot.generation != generation {
                            state.language.summary = "Discarded stale Java rename response".into();
                            continue;
                        }
                        begin_workspace_rename(state, snapshot, version, result);
                    }
                    if let Some((path, version, kind, result)) = document_edits {
                        if let Some(tab) = state.editor_tabs.iter_mut().find(|tab| tab.path == path) {
                            if state.language.is_document_current(&path, version, &tab.document.text) {
                                match result {
                                    Ok(edits) if !edits.is_empty() => {
                                        match tab.document.apply_lsp_edits(&edits) {
                                            Ok(()) => {
                                                tab.restore_document_cursor();
                                                state.language.summary = match kind {
                                                    crate::java::lsp::DocumentEditKind::Format => "Java file formatted".into(),
                                                    crate::java::lsp::DocumentEditKind::OrganizeImports => "Java imports organized".into(),
                                                };
                                            }
                                            Err(error) => state.language.summary = format!("Cannot apply Java edits: {error}"),
                                        }
                                    }
                                    Ok(_) => state.language.summary = match kind {
                                        crate::java::lsp::DocumentEditKind::Format => "Java file is already formatted".into(),
                                        crate::java::lsp::DocumentEditKind::OrganizeImports => "Java imports are already organized".into(),
                                    },
                                    Err(_) => {}
                                }
                            } else {
                                state.language.summary = "Discarded stale Java formatting result".into();
                            }
                        }
                    }
                }
            }
            AppEvent::RestartLanguage => restart_language(state),
            AppEvent::ShowPanel(panel) => {
                state.active_panel = panel;
                if !state.settings.show_tool_window {
                    state.settings.show_tool_window = true;
                    save_settings(state);
                }
            }
            AppEvent::SettingsChanged => {
                state.language.refresh_tooling(&state.settings);
                if !state.settings.show_tool_window {
                    state.terminal.close();
                }
                save_settings(state);
            }
            AppEvent::SettingsSaved { generation, result } => {
                if generation == state.settings_save_generation {
                    state.settings_save_pending = false;
                    state.settings_save_error = result
                        .err()
                        .map(|error| format!("Preferences have not been saved: {error}"));
                }
            }
            AppEvent::DiscoverJdks => {
                if let Err(error) = state
                    .jdk_discovery
                    .start_catalog(state.settings.jdks.clone())
                {
                    state.last_error = Some(error);
                } else {
                    state.status = "Discovering installed JDKs…".into();
                }
            }
            AppEvent::AddJdkPath { path, target } => {
                let minimum_jdt_ls_major = if matches!(
                    &target,
                    crate::java::jdk::JdkSelectionTarget::JdtLs
                ) {
                    state.settings.jdt_ls_minimum_major()
                } else {
                    0
                };
                if let Err(error) =
                    state
                        .jdk_discovery
                        .start_add(
                            path,
                            state.settings.jdks.clone(),
                            target,
                            minimum_jdt_ls_major,
                        )
                {
                    state.last_error = Some(error);
                } else {
                    state.status = "Validating JDK path…".into();
                }
            }
            AppEvent::JdksDiscovered(result) => {
                state.settings.jdks = result.catalog;
                if state.project.as_ref().is_some_and(|project| project.model.is_some()) {
                    // JDT LS receives the available JDK catalog and selected
                    // project runtime at startup; refresh it after catalog changes.
                    state.pending_language_restart = true;
                }
                if let Some(id) = result.selected_id {
                    match result.target {
                        crate::java::jdk::JdkSelectionTarget::Catalog => {}
                        crate::java::jdk::JdkSelectionTarget::Global => {
                            state.settings.default_jdk_id = Some(id);
                            state.pending_language_restart = true;
                        }
                        crate::java::jdk::JdkSelectionTarget::JdtLs => {
                            let minimum = state.settings.jdt_ls_minimum_major();
                            if state.settings.jdks.iter().find(|jdk| jdk.id == id).is_some_and(|jdk| jdk.version.major >= minimum) {
                                state.settings.jdt_ls_jdk_id = Some(id);
                                state.pending_language_restart = true;
                            } else {
                                state.last_error = Some(format!("{} JDT LS requires Java {minimum} or newer", if state.settings.jdt_ls_executable.is_some() { "Custom" } else { "Bundled" }));
                            }
                        }
                        crate::java::jdk::JdkSelectionTarget::Project(root) => {
                            state.settings.set_project_jdk(root, Some(id));
                            state.pending_language_restart = true;
                        }
                    }
                }
                if let Some(error) = result.error {
                    state.last_error = Some(error);
                }
                state.language.refresh_tooling(&state.settings);
                state.status = format!(
                    "JDK catalog updated: {} installation(s)",
                    state.settings.jdks.len()
                );
                save_settings(state);
            }
            AppEvent::Maven(action) => match action {
                MavenAction::ToggleCollapsed => state.maven_collapsed = !state.maven_collapsed,
                MavenAction::Reload { root } => {
                    if !state.quit_requested && state.maven_view_root.as_ref() == Some(&root) {
                        reload_maven_view(state);
                    }
                }
                MavenAction::ToggleProfile { root, profile } => {
                    if state.quit_requested
                        || state.maven_view_root.as_ref() != Some(&root)
                        || profile == "local"
                        || !state
                            .maven_view
                            .as_ref()
                            .is_some_and(|view| view.profiles.iter().any(|item| item.id == profile))
                    {
                        continue;
                    }
                    state.maven_profiles_user_modified = true;
                    if !state.maven_profiles.remove(&profile) {
                        state.maven_profiles.insert(profile);
                    }
                    reload_maven_view(state);
                }
                MavenAction::SelectModule { root, module } => {
                    if state.maven_view_root.as_ref() == Some(&root)
                        && state.maven_view.as_ref().is_some_and(|view| {
                            find_maven_module(&view.root_module, module.as_deref()).is_some()
                        })
                    {
                        state.maven_selected_module = module;
                    }
                }
                MavenAction::RunTask {
                    root,
                    generation,
                    module,
                    goal,
                } => {
                    if state.pending_run.is_some() {
                        state.last_error = Some("Finish or cancel the pending Run before starting another build".into());
                        continue;
                    }
                    if state.quit_requested
                        || state.maven_view_root.as_ref() != Some(&root)
                        || generation != state.maven_generation
                        || state.maven_loading
                        || state.maven_error.is_some()
                    {
                        continue;
                    }
                    let valid_task = state
                        .maven_view
                        .as_ref()
                        .and_then(|view| find_maven_module(&view.root_module, module.as_deref()))
                        .is_some_and(|owner| {
                            owner.lifecycle.iter().any(|task| task.goal == goal)
                                || owner
                                    .plugins
                                    .iter()
                                    .any(|plugin| plugin.tasks.iter().any(|task| task.goal == goal))
                        });
                    if !valid_task {
                        continue;
                    }
                    state.maven_selected_module = module.clone();
                    let request = (|| -> Result<_, String> {
                        let project = state.project.as_ref().ok_or("Open a Maven project first")?;
                        let model = project
                            .model
                            .as_ref()
                            .filter(|model| model.kind == ProjectKind::Maven && model.root == root)
                            .ok_or("The Maven project has changed")?
                            .clone();
                        let jdk = configured_maven_jdk(state, &project.root)?;
                        let mut profiles: Vec<_> = state.maven_profiles.iter().cloned().collect();
                        profiles.sort();
                        let mut disabled_profiles: Vec<_> = state
                            .maven_view
                            .as_ref()
                            .into_iter()
                            .flat_map(|view| &view.profiles)
                            .filter(|profile| {
                                profile.id != "local" && !state.maven_profiles.contains(&profile.id)
                            })
                            .map(|profile| profile.id.clone())
                            .collect();
                        disabled_profiles.sort();
                        Ok(crate::java::build::MavenTaskRequest {
                            project: model,
                            goal,
                            module,
                            profiles,
                            disabled_profiles,
                            jdk,
                        })
                    })();
                    match request {
                        Ok(request) => match state
                            .build
                            .start_maven_task(request, state.settings.maven_executable.as_deref())
                        {
                            Ok(()) => {
                                state.status = state.build.summary.clone();
                                state.enqueue(AppEvent::ShowPanel(super::ActivePanel::Build));
                            }
                            Err(error) => state.last_error = Some(error.to_string()),
                        },
                        Err(error) => state.last_error = Some(error),
                    }
                }
            },
            AppEvent::Gradle(action) => match action {
                super::events::GradleAction::ToggleCollapsed => {
                    state.gradle_collapsed = !state.gradle_collapsed;
                }
                super::events::GradleAction::Reload { root } => {
                    if !state.quit_requested && state.gradle_task_root.as_ref() == Some(&root) {
                        reload_gradle_tasks(state);
                    }
                }
                super::events::GradleAction::Cancel { root } => {
                    if state.gradle_task_root.as_ref() == Some(&root) && state.gradle_task_loading {
                        state.gradle_task_loader.cancel();
                        state.gradle_task_generation = state.gradle_task_generation.wrapping_add(1);
                        state.gradle_task_loading = false;
                        state.gradle_task_loaded = false;
                        state.gradle_tasks.clear();
                        state.gradle_task_error = None;
                    }
                }
                super::events::GradleAction::SelectProject { root, project_path } => {
                    if state.quit_requested || state.gradle_task_root.as_ref() != Some(&root) {
                        continue;
                    }
                    let valid = state.project.as_ref().and_then(|project| project.model.as_ref())
                        .is_some_and(|model| model.kind == ProjectKind::Gradle && model.root == root
                            && project_path.as_ref().is_none_or(|path| model.modules.iter().any(|module| &module.root == path)));
                    if valid {
                        state.gradle_selected_project = project_path;
                        reload_gradle_tasks(state);
                    }
                }
                super::events::GradleAction::RunTask { root, generation, project_path, task_path } => {
                    if state.pending_run.is_some() {
                        state.last_error = Some("Finish or cancel the pending Run before starting another build".into());
                        continue;
                    }
                    if state.quit_requested || state.gradle_task_root.as_ref() != Some(&root)
                        || generation != state.gradle_task_generation
                        || state.gradle_selected_project != project_path
                        || state.gradle_task_loading || state.gradle_task_error.is_some()
                        || !state.gradle_tasks.iter().any(|task| task.path == task_path)
                        || state.build.running()
                    {
                        continue;
                    }
                    let request = (|| -> Result<_, String> {
                        let project = state.project.as_ref().ok_or("Open a Gradle project first")?;
                        let model = project.model.as_ref()
                            .filter(|model| model.kind == ProjectKind::Gradle && model.root == root)
                            .ok_or("The Gradle project has changed")?.clone();
                        let jdk = state.settings.selected_jdk(&project.root, None)?.clone();
                        Ok(crate::java::build::GradleTaskRequest { project: model, task_path, jdk })
                    })();
                    match request {
                        Ok(request) => match state.build.start_gradle_task(request, state.settings.gradle_executable.as_deref()) {
                            Ok(()) => {
                                state.status = state.build.summary.clone();
                                state.enqueue(AppEvent::ShowPanel(super::ActivePanel::Build));
                            }
                            Err(error) => state.last_error = Some(error.to_string()),
                        },
                        Err(error) => state.last_error = Some(error),
                    }
                }
            },
            AppEvent::GradleTasksCompleted(event) => {
                if state.gradle_task_root.as_ref() != Some(&event.root)
                    || state.gradle_selected_project != event.project_path
                    || state.gradle_task_generation != event.generation
                {
                    continue;
                }
                state.gradle_task_loading = false;
                match event.result {
                    Ok(tasks) => {
                        state.gradle_tasks = tasks;
                        state.gradle_task_loaded = true;
                        state.gradle_task_error = None;
                    }
                    Err(error) if error == "Gradle task report cancelled" => {}
                    Err(error) => {
                        state.gradle_tasks.clear();
                        state.gradle_task_loaded = true;
                        state.gradle_task_error = Some(error);
                    }
                }
            }
            AppEvent::MavenViewCompleted {
                root,
                generation,
                result,
                cache_error,
            } => {
                if state.maven_view_root.as_ref() != Some(&root)
                    || generation != state.maven_generation
                {
                    continue;
                }
                state.maven_loading = false;
                state.maven_cache_error = cache_error;
                match result {
                    Ok(view) => {
                        sync_maven_profiles(state, &view);
                        if find_maven_module(
                            &view.root_module,
                            state.maven_selected_module.as_deref(),
                        )
                        .is_none()
                        {
                            state.maven_selected_module = None;
                        }
                        state.maven_view = Some(view);
                        state.maven_error = None;
                    }
                    Err(error) => state.maven_error = Some(error),
                }
            }
            AppEvent::MavenCacheLoaded {
                root,
                generation,
                result,
                current_pom_fingerprint,
            } => {
                if state.maven_view_root.as_ref() != Some(&root)
                    || generation != state.maven_cache_generation
                {
                    continue;
                }
                state.maven_cache_loading = false;
                match result {
                    Ok(Some(cached)) => {
                        if state.maven_view.is_none() {
                            state.maven_view = Some(cached.view);
                            state.maven_error = None;
                        }
                        if let Some(view) = state.maven_view.clone() {
                            sync_maven_profiles(state, &view);
                        }
                        match current_pom_fingerprint {
                            Ok(current)
                                if cached_maven_view_is_stale(
                                    Some(&cached.pom_fingerprint),
                                    Some(&current),
                                ) =>
                            {
                                if !state.maven_loading {
                                    reload_maven_view(state);
                                }
                            }
                            Ok(_) => {}
                            Err(error) => state.maven_cache_error = Some(error),
                        }
                    }
                    Ok(None) => {
                        if let Err(error) = current_pom_fingerprint {
                            state.maven_cache_error = Some(error);
                        }
                    }
                    Err(error) => state.maven_cache_error = Some(error),
                }
            }
            AppEvent::MavenPomsChanged(event) => {
                if !maven_pom_event_is_current(
                    state.maven_view_root.as_deref(),
                    state.maven_project_generation,
                    &event.root,
                    event.project_generation,
                ) {
                    continue;
                }
                match event.model {
                    Ok(model) if model.kind == ProjectKind::Maven => {
                        if let Some(project) = &mut state.project {
                            project.kind = model.kind;
                            project.model = Some(model);
                            project.discovery_error = None;
                        }
                        state.maven_error = None;
                        reload_maven_view(state);
                    }
                    Ok(model) => {
                        if let Some(project) = &mut state.project {
                            project.kind = model.kind;
                            project.model = Some(model);
                            project.discovery_error = None;
                        }
                        retire_maven_watcher(state);
                        state.maven_view_root = None;
                        state.maven_view = None;
                        state.maven_profiles.clear();
                        state.maven_profiles_user_modified = false;
                        state.maven_loading = false;
                        state.maven_view_loader.cancel();
                        state.maven_generation = state.maven_generation.wrapping_add(1);
                    }
                    Err(error) => {
                        if let Some(project) = &mut state.project {
                            project.discovery_error = Some(error.clone());
                        }
                        state.maven_view_loader.cancel();
                        state.maven_generation = state.maven_generation.wrapping_add(1);
                        state.maven_loading = false;
                        state.maven_error = Some(error);
                    }
                }
            }
            AppEvent::Status(message) => state.status = message,
            AppEvent::Error(message) => state.last_error = Some(message),
            AppEvent::Build(action) => {
                use super::events::BuildAction;
                use crate::java::build::{BuildRequest, BuildTask};
                if matches!(action, BuildAction::Cancel) {
                    state.build.cancel();
                    if state.pending_run.as_ref().is_some_and(|pending| pending.stage == super::PendingRunStage::Building) {
                        state.pending_run = None;
                        state.run.summary = "Build cancelled — Run was not started".into();
                    }
                    continue;
                }
                if state.quit_requested {
                    continue;
                }
                if state.pending_run.is_some() {
                    state.last_error = Some("Finish or cancel the pending Run before starting another build".into());
                    continue;
                }
                let request = (|| -> Result<_, String> {
                    let project = state.project.as_ref().ok_or("Open a Java project first")?;
                    let model = project
                        .model
                        .as_ref()
                        .ok_or("Build discovery has no Maven or Gradle model")?
                        .clone();
                    if model.kind == crate::project::ProjectKind::Unknown {
                        return Err("This directory is not a Maven or Gradle project; build actions are unavailable".into());
                    }
                    let jdk = state.settings.selected_jdk(&project.root, None)?.clone();
                    let executable = match model.kind {
                        crate::project::ProjectKind::Maven => {
                            state.settings.maven_executable.clone()
                        }
                        crate::project::ProjectKind::Gradle => {
                            state.settings.gradle_executable.clone()
                        }
                        crate::project::ProjectKind::Unknown => None,
                    };
                    let (task, module, test_task, test_filter) = match action {
                        BuildAction::Build => (BuildTask::Build, None, None, None),
                        BuildAction::Test => (BuildTask::Test, None, None, None),
                        BuildAction::Clean => (BuildTask::Clean, None, None, None),
                        BuildAction::RunTest { class_name, module_root, test_task, integration } => {
                            if model.kind == crate::project::ProjectKind::Maven {
                                let module = if module_root == model.root {
                                    None
                                } else {
                                    Some(module_root.strip_prefix(&model.root)
                                        .map_err(|_| "The test module is outside the Maven reactor")?.to_path_buf())
                                };
                                (BuildTask::Test, module, Some(if integration { "verify".into() } else { "test".into() }), Some(class_name))
                            } else if model.kind == crate::project::ProjectKind::Gradle {
                                let task_path = if module_root == model.root {
                                    format!(":{test_task}")
                                } else {
                                    let module = model.modules.iter().find(|module| module.root == module_root)
                                        .ok_or("The test module is no longer part of this Gradle project")?;
                                    format!("{}:{test_task}", module.name)
                                };
                                (BuildTask::Test, None, Some(task_path), Some(class_name))
                            } else {
                                return Err("Open a Maven or Gradle project to rerun a test".into());
                            }
                        }
                        BuildAction::Cancel => unreachable!(),
                    };
                    Ok((
                        BuildRequest {
                            project: model,
                            task,
                            jdk,
                            module,
                            test_task,
                            test_filter,
                        },
                        executable,
                    ))
                })();
                match request {
                    Ok((request, executable)) => {
                        match state.build.start(request, executable.as_deref()) {
                            Ok(()) => {
                                state.status = state.build.summary.clone();
                                state.enqueue(AppEvent::ShowPanel(super::ActivePanel::Build));
                            }
                            Err(error) => state.last_error = Some(error.to_string()),
                        }
                    }
                    Err(error) => state.last_error = Some(error),
                }
            }
            AppEvent::Run(action) => {
                use super::events::RunAction;
                let Some(project) = &state.project else {
                    continue;
                };
                match action {
                    RunAction::Stop => {
                        if state.pending_run.as_ref().is_some_and(|pending| pending.stage == super::PendingRunStage::Building) {
                            state.build.cancel();
                            state.pending_run = None;
                            state.run.summary = "Build cancelled — Run was not started".into();
                        } else {
                            state.pending_run = None;
                            state.run.cancel();
                        }
                    }
                    RunAction::CancelPendingStart => state.pending_run = None,
                    RunAction::ConfirmSaveAndRun => confirm_pending_run(state),
                    RunAction::BuildCompleted(successful) => finish_pending_run_build(state, successful),
                    RunAction::Input(line) => {
                        if let Err(error) = state.run.send_input(&line) {
                            state.run.input_line = line;
                            state.run.input_error = Some(error);
                        }
                    }
                    RunAction::Save(config) => {
                        if state.run.loading || state.run.saving {
                            continue;
                        }
                        let selected = config.id.clone();
                        match state.run.configurations_after_save(config) {
                            Ok(configurations) => {
                                state.run_configuration_generation =
                                    state.run_configuration_generation.wrapping_add(1);
                                match state.run_configuration_loader.save(
                                    project.root.clone(),
                                    state.run_configuration_generation,
                                    configurations,
                                    Some(selected),
                                ) {
                                    Ok(()) => {
                                        state.run.saving = true;
                                        state.run.configuration_error = None;
                                    }
                                    Err(error) => state.run.configuration_error = Some(error),
                                }
                            }
                            Err(error) => state.run.configuration_error = Some(error.to_string()),
                        }
                    }
                    RunAction::Delete => {
                        if state.run.loading || state.run.saving {
                            continue;
                        }
                        match state.run.configurations_after_delete() {
                            Ok(configurations) => {
                                let selected =
                                    configurations.first().map(|config| config.id.clone());
                                state.run_configuration_generation =
                                    state.run_configuration_generation.wrapping_add(1);
                                match state.run_configuration_loader.save(
                                    project.root.clone(),
                                    state.run_configuration_generation,
                                    configurations,
                                    selected,
                                ) {
                                    Ok(()) => {
                                        state.run.saving = true;
                                        state.run.configuration_error = None;
                                    }
                                    Err(error) => state.run.configuration_error = Some(error),
                                }
                            }
                            Err(error) => state.run.configuration_error = Some(error.to_string()),
                        }
                    }
                    RunAction::Start => {
                        state.active_panel = super::ActivePanel::Run;
                        state.settings.show_tool_window = true;
                        begin_run_flow(state);
                    }
                }
            }
            AppEvent::RunConfigurationsLoaded(event) => {
                if event.generation == state.run_configuration_generation
                    && state
                        .project
                        .as_ref()
                        .is_some_and(|project| project.root == event.root)
                {
                    if event.saved {
                        state.run.finish_save(event.result, event.selected);
                    } else {
                        state.run.finish_project_load(event.result);
                    }
                }
            }
            AppEvent::Debugger(action) => {
                use super::events::DebuggerAction;
                match action {
                    DebuggerAction::Stop => state.debugger.cancel(),
                    DebuggerAction::Start => {
                        state.active_panel = super::ActivePanel::Debugger;
                        state.settings.show_tool_window = true;
                        let result = (|| {
                            if state.pending_language_restart {
                                return Err("Wait for Java intelligence to restart".to_owned());
                            }
                            let project = state
                                .project
                                .as_ref()
                                .ok_or_else(|| "Open a Java project first".to_owned())?;
                            let model = project
                                .model
                                .as_ref()
                                .filter(|model| model.kind != crate::project::ProjectKind::Unknown)
                                .ok_or_else(|| "Open a Maven or Gradle project first".to_owned())?;
                            let config = state
                                .run
                                .selected()
                                .cloned()
                                .ok_or_else(|| "Create a run configuration first".to_owned())?;
                            let jdk = state.settings.run_jdk(&project.root, &config)?;
                            let bundle =
                                state.language.tooling.debug_bundle.clone().ok_or_else(|| {
                                    state.language.tooling.debug_error.clone().unwrap_or_else(
                                        || "Java Debug Server is unavailable".into(),
                                    )
                                })?;
                            let resolver = state
                                .language
                                .debug_adapter_resolver()
                                .map_err(|error| error.to_string())?;
                            state
                                .debugger
                                .start(
                                    crate::debugger::DebugService::new(resolver, bundle),
                                    &config,
                                    model,
                                    jdk,
                                )
                                .map_err(|error| error.to_string())
                        })();
                        if let Err(error) = result {
                            state.debugger.summary = error;
                        }
                    }
                    action => crate::debugger::handle_action(&mut state.debugger, action),
                }
            }
        }
    }
    advance_pending_run_saves(state);
    advance_pending_run_launch(state);
    advance_quit(state);
}

fn sync_maven_profiles(state: &mut AppState, view: &crate::project::maven_view::MavenView) {
    if !state.maven_profiles_user_modified {
        state.maven_profiles = view
            .profiles
            .iter()
            .filter(|profile| profile.id == "local" || profile.active_by_default)
            .map(|profile| profile.id.clone())
            .collect();
        return;
    }

    let available: std::collections::HashSet<_> =
        view.profiles.iter().map(|profile| profile.id.as_str()).collect();
    state
        .maven_profiles
        .retain(|profile| available.contains(profile.as_str()));
    if available.contains("local") {
        state.maven_profiles.insert("local".into());
    }
}

fn reload_maven_view(state: &mut AppState) {
    state.maven_generation = state.maven_generation.wrapping_add(1);
    state.maven_loading = false;
    state.maven_error = None;
    let request = (|| -> Result<_, String> {
        let project = state.project.as_ref().ok_or("Open a Maven project first")?;
        let model = project
            .model
            .as_ref()
            .filter(|model| model.kind == ProjectKind::Maven)
            .ok_or("Open a Maven project first")?
            .clone();
        if let Err(error) = state
            .language
            .refresh_maven_project(crate::project::maven_watch::project_pom_paths(&model))
        {
            // Java-language refresh is auxiliary; it must not block the Maven
            // view refresh requested by the user or POM watcher.
            state.language.summary = format!("Maven language refresh unavailable: {error}");
        }
        let jdk = configured_maven_jdk(state, &project.root)?;
        let mut profiles: Vec<_> = state.maven_profiles.iter().cloned().collect();
        profiles.sort();
        Ok(MavenViewRequest {
            project: model,
            executable: state.settings.maven_executable.clone(),
            jdk,
            profiles,
            profiles_user_modified: state.maven_profiles_user_modified,
            generation: state.maven_generation,
        })
    })();
    match request {
        Ok(request) => match state.maven_view_loader.start(request) {
            Ok(()) => state.maven_loading = true,
            Err(error) => state.maven_error = Some(error),
        },
        Err(error) => state.maven_error = Some(error),
    }
}

fn reload_gradle_tasks(state: &mut AppState) {
    state.gradle_task_loader.cancel();
    state.gradle_task_generation = state.gradle_task_generation.wrapping_add(1);
    state.gradle_task_loading = false;
    state.gradle_task_loaded = false;
    state.gradle_tasks.clear();
    state.gradle_task_error = None;
    let request = (|| -> Result<_, String> {
        let project = state.project.as_ref().ok_or("Open a Gradle project first")?;
        let model = project.model.as_ref()
            .filter(|model| model.kind == ProjectKind::Gradle && Some(&model.root) == state.gradle_task_root.as_ref())
            .ok_or("Open a Gradle project first")?.clone();
        if let Some(module) = &state.gradle_selected_project
            && !model.modules.iter().any(|item| &item.root == module)
        {
            return Err("Selected directory is not a discovered Gradle project".into());
        }
        let jdk = state.settings.selected_jdk(&project.root, None)?.clone();
        Ok((model, state.gradle_selected_project.clone(), state.settings.gradle_executable.clone(), jdk))
    })();
    match request {
        Ok((model, module, executable, jdk)) => match state.gradle_task_loader.start(
            model, module, executable, jdk, state.gradle_task_generation,
        ) {
            Ok(()) => state.gradle_task_loading = true,
            Err(error) => state.gradle_task_error = Some(error),
        },
        Err(error) => state.gradle_task_error = Some(error),
    }
}

fn retire_maven_watcher(state: &mut AppState) {
    if let Some(mut watcher) = state.maven_pom_watcher.take() {
        watcher.request_cancel();
        state.retiring_maven_watchers.push(watcher);
    }
}

fn retire_project_tree_watcher(state: &mut AppState) {
    if let Some(mut watcher) = state.project_tree_watcher.take() {
        watcher.request_cancel();
        state.retiring_project_tree_watchers.push(watcher);
    }
}

fn cached_maven_view_is_stale(cached: Option<&str>, current: Option<&str>) -> bool {
    matches!((cached, current), (Some(cached), Some(current)) if cached != current)
}

fn maven_pom_event_is_current(
    current_root: Option<&std::path::Path>,
    current_generation: u64,
    event_root: &std::path::Path,
    event_generation: u64,
) -> bool {
    current_root == Some(event_root) && current_generation == event_generation
}

fn find_maven_module<'a>(
    module: &'a MavenModuleView,
    path: Option<&std::path::Path>,
) -> Option<&'a MavenModuleView> {
    if module.relative_path.as_deref() == path {
        return Some(module);
    }
    module
        .modules
        .iter()
        .find_map(|child| find_maven_module(child, path))
}

fn configured_maven_jdk(
    state: &AppState,
    root: &std::path::Path,
) -> Result<crate::java::jdk::JdkInstallation, String> {
    let id = state
        .settings
        .project_jdk_id(root)
        .or(state.settings.default_jdk_id.as_deref())
        .ok_or("Choose a JDK in Settings or Project Structure")?;
    state
        .settings
        .jdks
        .iter()
        .find(|jdk| jdk.id == id)
        .cloned()
        .ok_or_else(|| format!("Selected JDK {id} is no longer registered; select a replacement"))
}

#[cfg(test)]
mod maven_cache_tests {
    use super::{cached_maven_view_is_stale, maven_pom_event_is_current};
    use std::path::Path;

    #[test]
    fn no_cache_does_not_trigger_automatic_maven_resolution() {
        assert!(!cached_maven_view_is_stale(None, Some("current")));
    }

    #[test]
    fn unchanged_cache_does_not_trigger_automatic_maven_resolution() {
        assert!(!cached_maven_view_is_stale(Some("same"), Some("same")));
    }

    #[test]
    fn changed_pom_fingerprint_triggers_automatic_maven_resolution() {
        assert!(cached_maven_view_is_stale(Some("old"), Some("new")));
    }

    #[test]
    fn stale_watcher_events_are_ignored_after_project_switch() {
        assert!(!maven_pom_event_is_current(
            Some(Path::new("/current")),
            3,
            Path::new("/old"),
            2
        ));
        assert!(!maven_pom_event_is_current(
            Some(Path::new("/same")),
            4,
            Path::new("/same"),
            3
        ));
        assert!(maven_pom_event_is_current(
            Some(Path::new("/same")),
            4,
            Path::new("/same"),
            4
        ));
    }
}

fn active_path(state: &AppState) -> Option<std::path::PathBuf> {
    state
        .active_tab
        .and_then(|index| state.editor_tabs.get(index))
        .map(|tab| tab.path.clone())
}

fn begin_run_flow(state: &mut AppState) {
    if state.pending_run.is_some() || state.run.has_workers() {
        return;
    }
    if state.build.running() {
        state.run.summary = "Wait for the current build to finish before running".into();
        return;
    }
    let Some(project) = state.project.as_ref() else {
        state.run.summary = "Open a Maven or Gradle project first".into();
        return;
    };
    let Some(model) = project.model.as_ref()
        .filter(|model| model.kind != crate::project::ProjectKind::Unknown).cloned()
    else {
        state.run.summary = "Open a Maven or Gradle project first".into();
        return;
    };
    let root = project.root.clone();
    let Some(config) = state.run.selected().cloned() else {
        state.run.summary = "Create a run configuration first".into();
        return;
    };
    if let Err(error) = state.language.classpath_resolver() {
        state.run.summary = error.to_string();
        return;
    }
    let jdk = match state.settings.run_jdk(&root, &config) {
        Ok(jdk) => jdk.clone(),
        Err(error) => { state.run.summary = error; return; }
    };
    let save_paths: HashSet<_> = state.editor_tabs.iter()
        .filter(|tab| tab.path.starts_with(&root)
            && (tab.document.is_dirty() || tab.saving || tab.pending_rename_to.is_some()))
        .map(|tab| tab.path.clone()).collect();
    let stage = if save_paths.is_empty() { super::PendingRunStage::Building }
        else { super::PendingRunStage::ConfirmSave };
    state.pending_run = Some(super::PendingRun {
        root,
        project_generation: state.project_open_generation,
        configuration_generation: state.run_configuration_generation,
        model,
        config,
        jdk,
        stage,
        save_paths,
    });
    if state.pending_run.as_ref().is_some_and(|pending| pending.stage == super::PendingRunStage::Building) {
        start_pending_run_build(state);
    }
}

fn confirm_pending_run(state: &mut AppState) {
    let Some(pending) = state.pending_run.as_mut() else { return; };
    if pending.stage != super::PendingRunStage::ConfirmSave { return; }
    pending.stage = super::PendingRunStage::Saving;
    let paths: Vec<_> = pending.save_paths.iter().cloned().collect();
    for path in paths {
        let Some(tab) = state.editor_tabs.iter_mut().find(|tab| tab.path == path) else {
            state.pending_run = None;
            state.run.summary = "A project file closed before it could be saved".into();
            return;
        };
        if tab.saving {
            if tab.document.is_dirty() { tab.save_again = true; }
            continue;
        }
        if !save_file(state, &path) {
            state.pending_run = None;
            state.run.summary = "Could not save project files; Run was not started".into();
            return;
        }
    }
    advance_pending_run_saves(state);
}

fn advance_pending_run_saves(state: &mut AppState) {
    let Some(pending) = state.pending_run.as_ref() else { return; };
    if pending.stage != super::PendingRunStage::Saving { return; }
    if !run_context_is_current(state, pending) {
        state.pending_run = None;
        state.run.summary = "Project or run configuration changed; Run was cancelled".into();
        return;
    }
    let root = pending.root.clone();
    let newly_dirty: Vec<_> = state.editor_tabs.iter()
        .filter(|tab| tab.path.starts_with(&root)
            && (tab.document.is_dirty() || tab.saving || tab.pending_rename_to.is_some()))
        .map(|tab| tab.path.clone()).collect();
    if let Some(pending) = state.pending_run.as_mut() {
        pending.save_paths.extend(newly_dirty);
    }
    let Some(pending) = state.pending_run.as_ref() else { return; };
    let paths: Vec<_> = pending.save_paths.iter().cloned().collect();
    let mut waiting = false;
    for path in paths {
        let Some(tab) = state.editor_tabs.iter().find(|tab| tab.path == path) else {
            state.pending_run = None;
            state.run.summary = "A project file closed before it could be saved".into();
            return;
        };
        if tab.saving { waiting = true; continue; }
        if tab.document.is_dirty() || tab.pending_rename_to.is_some() {
            if !save_file(state, &path) {
                state.pending_run = None;
                state.run.summary = "Could not save project files; Run was not started".into();
                return;
            }
            waiting = true;
        }
    }
    if !waiting {
        if let Some(pending) = state.pending_run.as_mut() {
            pending.stage = super::PendingRunStage::Building;
        }
        start_pending_run_build(state);
    }
}

fn start_pending_run_build(state: &mut AppState) {
    let Some(pending) = state.pending_run.as_ref() else { return; };
    if pending.stage != super::PendingRunStage::Building { return; }
    if !run_context_is_current(state, pending) {
        state.pending_run = None;
        state.run.summary = "Project or run configuration changed; Run was cancelled".into();
        return;
    }
    let executable = match pending.model.kind {
        crate::project::ProjectKind::Maven => state.settings.maven_executable.clone(),
        crate::project::ProjectKind::Gradle => state.settings.gradle_executable.clone(),
        crate::project::ProjectKind::Unknown => None,
    };
    let request = crate::java::build::BuildRequest {
        project: pending.model.clone(),
        task: crate::java::build::BuildTask::Build,
        jdk: pending.jdk.clone(),
        module: None,
        test_task: None,
        test_filter: None,
    };
    match state.build.start(request, executable.as_deref()) {
        Ok(()) => {
            state.status = state.build.summary.clone();
            state.enqueue(AppEvent::ShowPanel(super::ActivePanel::Build));
        }
        Err(error) => {
            state.pending_run = None;
            state.run.summary = format!("Build could not start: {error}");
        }
    }
}

fn finish_pending_run_build(state: &mut AppState, successful: bool) {
    let Some(pending) = state.pending_run.as_ref() else { return; };
    if pending.stage != super::PendingRunStage::Building { return; }
    if !successful {
        state.pending_run = None;
        state.run.summary = "Build failed or was cancelled — Run was not started".into();
        return;
    }
    if !run_context_is_current(state, &pending) {
        state.pending_run = None;
        state.run.summary = "Project or run configuration changed; Run was cancelled".into();
        return;
    }
    if let Some(pending) = state.pending_run.as_mut() {
        pending.stage = super::PendingRunStage::Launching;
    }
    advance_pending_run_launch(state);
}

fn advance_pending_run_launch(state: &mut AppState) {
    let Some(pending) = state.pending_run.as_ref() else { return; };
    if pending.stage != super::PendingRunStage::Launching { return; }
    if !run_context_is_current(state, pending) {
        state.pending_run = None;
        state.run.summary = "Project or run configuration changed; Run was cancelled".into();
        return;
    }
    if state.pending_language_restart { return; }
    let resolver = match state.language.classpath_resolver() {
        Ok(resolver) => resolver,
        Err(error) if error.to_string().contains("Wait for Java intelligence to start") => {
            if state.language.has_pending_start() || state.language.has_workers() {
                state.run.summary = "Build finished; waiting for Java intelligence…".into();
                return;
            }
            state.pending_run = None;
            state.run.summary = error.to_string();
            return;
        }
        Err(error) => {
            state.pending_run = None;
            state.run.summary = error.to_string();
            return;
        }
    };
    let pending = state.pending_run.take().expect("pending run checked above");
    if let Err(error) = state.run.start(
        crate::java::run_config::RunService::new(resolver),
        &pending.config,
        &pending.model,
        &pending.jdk,
    ) {
        state.run.summary = error.to_string();
    }
}

fn run_context_is_current(state: &AppState, pending: &super::PendingRun) -> bool {
    state.project.as_ref().is_some_and(|project| project.root == pending.root)
        && state.project_open_generation == pending.project_generation
        && state.run_configuration_generation == pending.configuration_generation
        && state.run.selected() == Some(&pending.config)
}

fn save_file(state: &mut AppState, path: &std::path::Path) -> bool {
    let Some(index) = state.editor_tabs.iter().position(|tab| tab.path == path) else {
        return false;
    };
    let tab = &mut state.editor_tabs[index];
    if tab.saving {
        tab.save_again = true;
        return true;
    }
    if !tab.document.is_dirty() && tab.pending_rename_to.is_none() {
        if tab.close_after_save {
            remove_tab(state, path);
        }
        return true;
    }
    match state.editor_io.save(tab.document.clone(), tab.pending_rename_to.clone()) {
        Ok(()) => {
            tab.saving = true;
            state.status = format!("Saving {}…", path.display());
            true
        }
        Err(error) => {
            tab.close_after_save = false;
            state.last_error = Some(error.clone());
            false
        }
    }
}

fn close_file(state: &mut AppState, path: &std::path::Path) {
    if state.pending_close.is_some() {
        return;
    }
    let Some(index) = state.editor_tabs.iter().position(|tab| tab.path == path) else {
        return;
    };
    let tab = &state.editor_tabs[index];
    if tab.document.is_dirty() || tab.saving {
        state.pending_close = Some(tab.path.clone());
        state.active_tab = Some(index);
    } else {
        remove_tab(state, path);
    }
}

fn remove_tab(state: &mut AppState, path: &std::path::Path) {
    let Some(index) = state.editor_tabs.iter().position(|tab| tab.path == path) else {
        return;
    };
    state.editor_tabs.remove(index);
    state.active_tab = match state.active_tab {
        _ if state.editor_tabs.is_empty() => None,
        Some(active) if active > index => Some(active - 1),
        Some(active) => Some(active.min(state.editor_tabs.len() - 1)),
        None => Some(0),
    };
    if state.pending_close.as_deref() == Some(path) {
        state.pending_close = None;
    }
}

fn begin_symbol_rename(state: &mut AppState) {
    let dialog = state.active_tab.and_then(|index| state.editor_tabs.get(index)).and_then(|tab| {
        if tab.path.extension().is_none_or(|extension| extension != "java") { return None; }
        Some(super::RenameSymbolDialog {
            path: tab.path.clone(), revision: tab.document.revision,
            position: tab.document.lsp_position(),
            old_name: tab.document.word_at_cursor()?,
            new_name: tab.document.word_at_cursor()?,
        })
    });
    match dialog {
        Some(dialog) => state.rename_symbol = Some(dialog),
        None => state.last_error = Some("Place the caret on a Java identifier before renaming".into()),
    }
}

fn begin_workspace_rename(
    state: &mut AppState,
    snapshot: super::PendingRenameRequest,
    version: i64,
    result: Result<Vec<crate::java::lsp::WorkspaceFileEdits>, String>,
) {
    let files = match result {
        Ok(files) => files,
        Err(error) => { state.last_error = Some(format!("Rename failed: {error}")); return; }
    }.into_iter().filter(|file| !file.edits.is_empty() || file.rename_to.is_some()).collect::<Vec<_>>();
    if files.is_empty() {
        state.language.summary = "Rename made no changes".into();
        return;
    }
    if !state.language.accepts_generation(snapshot.generation) {
        state.language.summary = "Discarded stale Java rename result".into();
        return;
    }
    if state.project.as_ref().is_none_or(|project| project.root != snapshot.root) {
        state.last_error = Some("The project changed before rename completed".into());
        return;
    }
    let Some(source) = state.editor_tabs.iter().find(|tab| tab.path == snapshot.source_path) else {
        state.last_error = Some("The Java source closed before rename completed".into());
        return;
    };
    if source.document.revision != snapshot.source_revision
        || source.document.lsp_position() != snapshot.position
        || !state.language.is_document_current(&snapshot.source_path, version, &source.document.text)
    {
        state.last_error = Some("The Java source changed; invoke Rename Symbol again".into());
        return;
    }
    if !state.pending_workspace_edits.is_empty() {
        state.last_error = Some("Another workspace edit is still loading; wait and retry".into());
        return;
    }
    let mut open_revisions = HashMap::new();
    let mut closed_paths = Vec::new();
    for file in &files {
        if !file.path.starts_with(&snapshot.root) {
            state.last_error = Some(format!("Rename target leaves the project: {}", file.path.display()));
            return;
        }
        if let Some(version) = file.version
            && snapshot.open_versions.get(&file.path) != Some(&version)
        {
            state.last_error = Some(format!("{} changed in Java intelligence while rename was running", file.path.display()));
            return;
        }
        let tab = state.editor_tabs.iter().find(|tab| tab.path == file.path);
        if let Some(revision) = snapshot.open_revisions.get(&file.path) {
            let Some(tab) = tab.filter(|tab| tab.document.revision == *revision) else {
                state.last_error = Some(format!("{} changed or closed while the rename was running", file.path.display()));
                return;
            };
            if tab.saving || tab.close_after_save {
                state.last_error = Some(format!("Wait for {} to finish saving or closing before renaming", file.path.display()));
                return;
            }
            open_revisions.insert(file.path.clone(), *revision);
        } else {
            if tab.is_some() {
                state.last_error = Some(format!("{} opened while the rename was running; retry", file.path.display()));
                return;
            }
            closed_paths.push(file.path.clone());
        }
    }
    state.next_workspace_edit_id = state.next_workspace_edit_id.wrapping_add(1).max(1);
    let request = state.next_workspace_edit_id;
    state.pending_workspace_edits.insert(request, super::PendingWorkspaceRename {
        root: snapshot.root, generation: snapshot.generation, source_path: snapshot.source_path,
        version, position: snapshot.position, source_revision: snapshot.source_revision,
        open_revisions, files,
    });
    if closed_paths.is_empty() {
        finish_workspace_rename(state, request, Ok(Vec::new()));
    } else if let Err(error) = state.editor_io.load_many(request, closed_paths) {
        state.pending_workspace_edits.remove(&request);
        state.last_error = Some(format!("Cannot load rename targets: {error}"));
    } else {
        state.status = "Loading files for Java symbol rename…".into();
    }
}

fn finish_workspace_rename(
    state: &mut AppState,
    request: u64,
    result: Result<Vec<crate::editor::Document>, String>,
) {
    let Some(pending) = state.pending_workspace_edits.remove(&request) else { return; };
    if !state.language.accepts_generation(pending.generation)
        || state.project.as_ref().is_none_or(|project| project.root != pending.root)
    {
        state.language.summary = "Discarded stale Java rename result".into();
        return;
    }
    let loaded = match result {
        Ok(documents) => documents,
        Err(error) => { state.last_error = Some(format!("Cannot load all files for rename: {error}")); return; }
    };
    let Some(source) = state.editor_tabs.iter().find(|tab| tab.path == pending.source_path) else {
        state.last_error = Some("The Java source closed before rename was applied".into());
        return;
    };
    if source.document.revision != pending.source_revision
        || source.document.lsp_position() != pending.position
        || !state.language.is_document_current(&pending.source_path, pending.version, &source.document.text)
    {
        state.last_error = Some("The Java source changed while rename targets loaded; try again".into());
        return;
    }
    for (path, revision) in &pending.open_revisions {
        if state.editor_tabs.iter().find(|tab| &tab.path == path)
            .is_none_or(|tab| tab.document.revision != *revision || tab.saving || tab.close_after_save)
        {
            state.last_error = Some("An open file changed while rename targets loaded; try again".into());
            return;
        }
    }
    let mut loaded: HashMap<_, _> = loaded.into_iter().map(|document| (document.path.clone(), document)).collect();
    let mut staged = Vec::with_capacity(pending.files.len());
    let mut staged_bytes = 0usize;
    for file in pending.files {
        let mut document = if let Some(tab) = state.editor_tabs.iter().find(|tab| tab.path == file.path) {
            if !pending.open_revisions.contains_key(&file.path) {
                state.last_error = Some("A rename target opened while the operation was loading; retry".into());
                return;
            }
            tab.document.clone()
        } else {
            if pending.open_revisions.contains_key(&file.path) {
                state.last_error = Some("A rename target closed while the operation was loading; retry".into());
                return;
            }
            let Some(document) = loaded.remove(&file.path) else {
                state.last_error = Some(format!("Rename target was not loaded: {}", file.path.display()));
                return;
            };
            document
        };
        if let Err(error) = document.apply_lsp_edits(&file.edits) {
            state.last_error = Some(format!("Cannot apply rename edits to {}: {error}", file.path.display()));
            return;
        }
        staged_bytes = match staged_bytes.checked_add(document.text.len()) {
            Some(bytes) if bytes <= crate::editor::MAX_WORKSPACE_EDIT_BYTES => bytes,
            _ => {
                state.last_error = Some(format!("Rename targets exceed the {} MiB workspace edit limit", crate::editor::MAX_WORKSPACE_EDIT_BYTES / (1024 * 1024)));
                return;
            }
        };
        if let Some(destination) = &file.rename_to {
            if !destination.starts_with(&pending.root) {
                state.last_error = Some(format!("Rename destination is unavailable: {}", destination.display()));
                return;
            }
            if document.path.parent() != destination.parent() {
                state.last_error = Some("Java file rename must stay in the same directory".into());
                return;
            }
        }
        staged.push((file.path, document, file.rename_to));
    }
    if !loaded.is_empty() {
        state.last_error = Some("The rename response changed while target files were loading".into());
        return;
    }
    let changed = staged.len();
    for (path, document, rename_to) in staged {
        if let Some(tab) = state.editor_tabs.iter_mut().find(|tab| tab.path == path) {
            tab.document = document;
            tab.pending_rename_to = rename_to;
            tab.refresh_after_workspace_edit();
        } else {
            let mut tab = EditorTab::new(document);
            tab.pending_rename_to = rename_to;
            state.editor_tabs.push(tab);
        }
    }
    state.active_tab = state.editor_tabs.iter().position(|tab| tab.path == pending.source_path);
    state.last_error = None;
    state.status = format!("Renamed Java symbol across {changed} file(s); save changed tabs to keep the edits");
    state.language.summary = format!("Rename applied to {changed} file(s)");
}

fn apply_project_replace(state: &mut AppState) {
    if !state.pending_workspace_replacements.is_empty() || !state.pending_workspace_edits.is_empty() {
        state.search.message = "Wait for the current workspace edit to finish".into();
        return;
    }
    let Some(project) = &state.project else {
        state.search.message = "Open a project before replacing files".into();
        return;
    };
    let Some(query) = state.search.preview_query.clone() else {
        state.search.message = "Run a fresh replacement preview first".into();
        return;
    };
    let Some(replacement) = state.search.preview_replacement.clone() else {
        state.search.message = "Run a fresh replacement preview first".into();
        return;
    };
    let selected_count = state.search.replace_files.iter().filter(|file| file.selected).count();
    if selected_count == 0 {
        state.search.message = "Select at least one file to replace".into();
        return;
    }
    if selected_count > 200 {
        state.search.message = "Select no more than 200 files at once".into();
        return;
    }
    let root = project.root.clone();
    let mut paths = HashSet::new();
    let mut open_revisions = HashMap::new();
    let mut closed_paths = Vec::new();
    for file in state.search.replace_files.iter().filter(|file| file.selected) {
        if !file.path.starts_with(&root) || !paths.insert(file.path.clone()) {
            state.search.message = "The preview contains an invalid or duplicate workspace path".into();
            return;
        }
        let tab = state.editor_tabs.iter().find(|tab| tab.path == file.path);
        match (file.open_revision, tab) {
            (Some(revision), Some(tab))
                if tab.document.revision == revision
                    && (file.recheck_disk || tab.document.text == file.source_text)
                    && !tab.saving
                    && !tab.close_after_save =>
            {
                open_revisions.insert(file.path.clone(), revision);
            }
            (Some(_), _) => {
                state.search.message = format!("{} changed or closed since preview; run the preview again", file.path.display());
                return;
            }
            (None, Some(_)) => {
                state.search.message = format!("{} opened since preview; run the preview again", file.path.display());
                return;
            }
            (None, None) => closed_paths.push(file.path.clone()),
        }
    }
    let files = state.search.take_selected_replace_files().into_iter().map(|file| {
        super::PendingWorkspaceReplaceFile {
            path: file.path,
            source_text: Some(file.source_text),
            open_revision: file.open_revision,
            recheck_disk: file.recheck_disk,
            match_count: file.match_count,
        }
    }).collect();
    state.next_workspace_edit_id = state.next_workspace_edit_id.wrapping_add(1).max(1);
    let request = state.next_workspace_edit_id;
    state.pending_workspace_replacements.insert(request, super::PendingWorkspaceReplace {
        root,
        project_generation: state.project_open_generation,
        query,
        replacement,
        files,
        open_revisions,
    });
    if closed_paths.is_empty() {
        finish_workspace_replace_load(state, request, Ok(Vec::new()));
    } else if let Err(error) = state.editor_io.load_many(request, closed_paths) {
        state.pending_workspace_replacements.remove(&request);
        state.search.message = format!("Cannot load replacement targets: {error}");
    } else {
        state.search.message = "Loading selected files for replacement…".into();
    }
}

fn finish_workspace_replace_load(
    state: &mut AppState,
    request: u64,
    result: Result<Vec<crate::editor::Document>, String>,
) {
    let Some(mut pending) = state.pending_workspace_replacements.remove(&request) else { return; };
    if state.project_open_generation != pending.project_generation
        || state.project.as_ref().is_none_or(|project| project.root != pending.root)
    {
        return;
    }
    let loaded = match result {
        Ok(documents) => documents,
        Err(error) => {
            state.search.message = format!("Cannot load replacement targets: {error}");
            return;
        }
    };
    let mut loaded: HashMap<_, _> = loaded.into_iter().map(|document| (document.path.clone(), document)).collect();
    let mut targets = Vec::with_capacity(pending.files.len());
    for file in &mut pending.files {
        let document = if let Some(revision) = file.open_revision {
            let Some(tab) = state.editor_tabs.iter().find(|tab| tab.path == file.path)
                .filter(|tab| tab.document.revision == revision && !tab.saving && !tab.close_after_save)
            else {
                state.search.message = format!("{} changed while loading targets; run the preview again", file.path.display());
                return;
            };
            tab.document.clone()
        } else {
            if state.editor_tabs.iter().any(|tab| tab.path == file.path) {
                state.search.message = format!("{} opened while loading targets; run the preview again", file.path.display());
                return;
            }
            let Some(document) = loaded.remove(&file.path) else {
                state.search.message = format!("Replacement target was not loaded: {}", file.path.display());
                return;
            };
            document
        };
        let Some(expected_text) = file.source_text.take() else {
            state.search.message = format!("Replacement preview for {} was already consumed", file.path.display());
            return;
        };
        targets.push(crate::editor::WorkspaceReplaceTarget {
            document,
            expected_text,
            expected_matches: file.match_count,
            recheck_disk: file.recheck_disk,
        });
    }
    if !loaded.is_empty() {
        state.search.message = "Replacement targets changed while loading; run the preview again".into();
        return;
    }
    let query = pending.query.text.clone();
    let replacement = pending.replacement.clone();
    let case_sensitive = pending.query.case_sensitive;
    state.pending_workspace_replacements.insert(request, pending);
    if let Err(error) = state.editor_io.replace_many(
        request,
        targets,
        query,
        replacement,
        case_sensitive,
    ) {
        state.pending_workspace_replacements.remove(&request);
        state.search.message = format!("Cannot prepare project replacement: {error}");
    } else {
        state.search.message = "Preparing replacements; editor files remain unsaved…".into();
    }
}

fn finish_workspace_replace(
    state: &mut AppState,
    request: u64,
    result: Result<Vec<(crate::editor::Document, usize)>, String>,
) {
    let Some(pending) = state.pending_workspace_replacements.remove(&request) else { return; };
    if state.project_open_generation != pending.project_generation
        || state.project.as_ref().is_none_or(|project| project.root != pending.root)
    {
        return;
    }
    let replacements = match result {
        Ok(replacements) => replacements,
        Err(error) => {
            state.search.message = error;
            return;
        }
    };
    for file in &pending.files {
        match pending.open_revisions.get(&file.path) {
            Some(revision) if state.editor_tabs.iter().any(|tab| tab.path == file.path
                && tab.document.revision == *revision && !tab.saving && !tab.close_after_save) => {}
            Some(_) => {
                state.search.message = format!("{} changed before replacement could be applied; no files were changed", file.path.display());
                return;
            }
            None if state.editor_tabs.iter().any(|tab| tab.path == file.path) => {
                state.search.message = format!("{} opened before replacement could be applied; no files were changed", file.path.display());
                return;
            }
            None => {}
        }
    }
    if replacements.len() != pending.files.len() {
        state.search.message = "Replacement worker returned an incomplete result; no files were changed".into();
        return;
    }
    let mut total_matches = 0usize;
    for (document, count) in &replacements {
        if !document.path.starts_with(&pending.root)
            || !pending.files.iter().any(|file| file.path == document.path)
        {
            state.search.message = "Replacement worker returned a file outside the preview; no files were changed".into();
            return;
        }
        total_matches = total_matches.saturating_add(*count);
    }
    let changed_files = replacements.len();
    for (document, _) in replacements {
        if let Some(tab) = state.editor_tabs.iter_mut().find(|tab| tab.path == document.path) {
            tab.document = document;
            tab.refresh_after_workspace_edit();
        } else {
            state.editor_tabs.push(EditorTab::new(document));
        }
    }
    state.active_tab = state.editor_tabs.len().checked_sub(1);
    state.last_error = None;
    state.search.clear_replace_preview();
    state.search.message = format!("Replaced {total_matches} matches in {changed_files} files; changes are unsaved");
    state.status = "Project replacement applied to editor buffers; save changed files to keep the edits".into();
}

fn editor_completed(state: &mut AppState, completion: EditorCompletion) {
    match completion {
        EditorCompletion::LoadedMany { request, result } => {
            if state.pending_workspace_replacements.contains_key(&request) {
                finish_workspace_replace_load(state, request, result);
            } else {
                finish_workspace_rename(state, request, result);
            }
        }
        EditorCompletion::ReplacedMany { request, result } => {
            finish_workspace_replace(state, request, result);
        }
        EditorCompletion::Loaded {
            requested_path,
            result,
        } => {
            state.loading_files.remove(&requested_path);
            match result {
                Ok(document) if !state.quit_requested => {
                    let index = if let Some(index) = state
                        .editor_tabs
                        .iter()
                        .position(|tab| tab.path == document.path)
                    {
                        index
                    } else {
                        state.editor_tabs.push(EditorTab::new(document));
                        state.editor_tabs.len() - 1
                    };
                    state.active_tab = Some(index);
                    if let Some(position) = state.pending_navigation.remove(&requested_path) {
                        navigate_tab_to_position(&mut state.editor_tabs[index], position);
                    }
                    state.status = format!("Opened {}", state.editor_tabs[index].path.display());
                }
                Ok(_) => {}
                Err(error) => {
                    state.pending_navigation.remove(&requested_path);
                    state.last_error = Some(error);
                }
            }
        }
        EditorCompletion::Saved {
            path,
            saved_path,
            snapshot,
            result,
        } => {
            if result.is_ok() {
                state.language.did_save(path.clone());
            }
            let Some(index) = state.editor_tabs.iter().position(|tab| tab.path == path) else {
                return;
            };
            let mut refresh_directory = None;
            let tab = &mut state.editor_tabs[index];
            tab.saving = false;
            let save_again = std::mem::take(&mut tab.save_again);
            match result {
                Ok(()) => {
                    // Only the exact snapshot written to disk becomes the clean baseline.
                    tab.document.saved_text = snapshot;
                    let mut current_path = path.clone();
                    if saved_path != path {
                        if let Some(pending) = state.pending_run.as_mut() {
                            if pending.save_paths.remove(&path) {
                                pending.save_paths.insert(saved_path.clone());
                            }
                        }
                        state.debugger.rename_breakpoint_source(&path, saved_path.clone());
                        tab.path = saved_path.clone();
                        tab.document.path = saved_path.clone();
                        tab.pending_rename_to = None;
                        current_path = saved_path.clone();
                        if state.pending_close.as_deref() == Some(path.as_path()) {
                            state.pending_close = Some(saved_path.clone());
                        }
                        refresh_directory = saved_path.parent().map(std::path::Path::to_path_buf);
                    }
                    let dirty = tab.document.is_dirty();
                    let close = tab.close_after_save && !dirty;
                    state.status = format!(
                        "Saved {}{}",
                        path.display(),
                        if dirty {
                            " — newer edits remain unsaved"
                        } else {
                            ""
                        }
                    );
                    if close {
                        remove_tab(state, &current_path);
                    } else if save_again && dirty {
                        if !save_file(state, &current_path)
                            && state.pending_run.as_ref().is_some_and(|pending| pending.save_paths.contains(&current_path))
                        {
                            state.pending_run = None;
                            state.run.summary = "Could not save project files; Run was not started".into();
                        }
                    } else if state.pending_close.as_deref() == Some(current_path.as_path()) && !dirty {
                        remove_tab(state, &current_path);
                    }
                }
                Err(error) => {
                    tab.close_after_save = false;
                    if state.pending_run.as_ref().is_some_and(|pending| pending.save_paths.contains(&path)) {
                        state.pending_run = None;
                        state.run.summary = "Saving project files failed; Run was not started".into();
                    }
                    state.last_error = Some(error);
                }
            }
            if let Some(directory) = refresh_directory {
                state.enqueue(AppEvent::RefreshProjectTree(directory));
            }
        }
        EditorCompletion::JavaClassCreated { root, directory, result } => {
            if state.project.as_ref().is_none_or(|project| project.root != root) { return; }
            match result {
                Ok(path) => {
                    state.status = format!("Created {}", path.display());
                    state.enqueue(AppEvent::RefreshProjectTree(directory));
                    if !state.quit_requested { state.enqueue(AppEvent::OpenFile(path)); }
                }
                Err(error) => state.last_error = Some(error),
            }
        }
    }
}

fn advance_quit(state: &mut AppState) {
    if !state.quit_requested || state.pending_close.is_some() {
        return;
    }
    if let Some(path) = state
        .editor_tabs
        .iter()
        .find(|tab| tab.document.is_dirty() || tab.saving)
        .map(|tab| tab.path.clone())
    {
        close_file(state, &path);
    } else {
        state.quit_approved = true;
    }
}

fn save_settings(state: &mut AppState) {
    if state.settings_writable {
        match state.settings_saver.start(state.settings.clone()) {
            Ok(generation) => {
                state.settings_save_generation = generation;
                state.settings_save_pending = true;
                state.settings_save_error = None;
            }
            Err(error) => {
                state.settings_save_pending = false;
                state.settings_save_error =
                    Some(format!("Preferences have not been saved: {error}"));
            }
        }
    }
}

/// Shared selector for settings, project defaults and run configurations.
fn browse_for_jdk(
    events: &mut Vec<AppEvent>,
    target: crate::java::jdk::JdkSelectionTarget,
) -> bool {
    let Some(path) = rfd::FileDialog::new()
        .set_title("Select JDK home or .jdk bundle")
        .pick_folder()
    else {
        return false;
    };
    events.push(AppEvent::AddJdkPath { path, target });
    true
}

pub fn jdk_selector(
    ui: &mut egui::Ui,
    salt: &str,
    selected: &mut Option<String>,
    jdks: &mut Vec<crate::java::jdk::JdkInstallation>,
    inherited_label: &str,
    empty_label: &str,
    browse_label: &str,
    minimum_major: u32,
    target: crate::java::jdk::JdkSelectionTarget,
    events: &mut Vec<AppEvent>,
    busy: bool,
) -> bool {
    let before = selected.clone();
    let label = selected
        .as_deref()
        .map(|id| {
            jdks.iter()
                .find(|jdk| jdk.id == id)
                .map(|jdk| jdk.label())
                .unwrap_or_else(|| format!("Missing JDK ({id})"))
        })
        .unwrap_or_else(|| empty_label.to_owned());
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt(salt)
            .selected_text(label)
            .show_ui(ui, |ui| {
                ui.selectable_value(selected, None, inherited_label);
                for jdk in jdks.iter() {
                    ui.add_enabled_ui(jdk.valid && jdk.version.major >= minimum_major, |ui| {
                        ui.selectable_value(selected, Some(jdk.id.clone()), jdk.label())
                            .on_hover_text(jdk.home.display().to_string());
                    });
                }
            });
        if ui
            .add_enabled(!busy, egui::Button::new(browse_label))
            .clicked()
        {
            let _ = browse_for_jdk(events, target.clone());
        }
    });
    before != *selected
}

fn jdk_catalog_controls(ui: &mut egui::Ui, state: &mut AppState) {
    ui.horizontal(|ui| {
        let label = if state.jdk_discovery.has_worker() {
            "Discovering JDKs…"
        } else {
            "Discover / refresh JDKs"
        };
        if ui
            .add_enabled(!state.jdk_discovery.has_worker(), egui::Button::new(label))
            .clicked()
        {
            state.enqueue(AppEvent::DiscoverJdks);
        }
        if ui.button("Browse…").clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Select JDK home or .jdk bundle")
                .pick_folder()
            {
                state.jdk_path_input = path.display().to_string();
            }
        }
    });
    if state.jdk_discovery.has_worker() {
        ui.spinner();
    }
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.jdk_path_input)
                .hint_text("JDK home or .jdk bundle path"),
        );
        if ui
            .add_enabled(
                !state.jdk_path_input.trim().is_empty() && !state.jdk_discovery.has_worker(),
                egui::Button::new("Add JDK"),
            )
            .clicked()
        {
            let path = std::path::PathBuf::from(state.jdk_path_input.trim());
            state.enqueue(AppEvent::AddJdkPath {
                path,
                target: crate::java::jdk::JdkSelectionTarget::Catalog,
            });
            state.jdk_path_input.clear();
        }
    });
}

pub fn jdk_settings_ui(ui: &mut egui::Ui, state: &mut AppState) {
    ui.heading("Java installations");
    jdk_catalog_controls(ui, state);
    let mut changed = false;
    let mut remove = None;
    egui::ScrollArea::vertical()
        .max_height(220.0)
        .show(ui, |ui| {
            for jdk in &state.settings.jdks {
                ui.horizontal(|ui| {
                    if jdk.valid {
                        ui.label(jdk.label());
                    } else {
                        ui.colored_label(ui.visuals().error_fg_color, jdk.label());
                    }
                    if ui.small_button("Remove").clicked() {
                        remove = Some(jdk.id.clone());
                    }
                });
                ui.small(jdk.home.display().to_string());
                ui.separator();
            }
        });
    if let Some(id) = remove {
        state.settings.jdks.retain(|jdk| jdk.id != id);
        // Retain selected ids: affected projects must explicitly pick a replacement.
        changed = true;
        state.pending_language_restart = true;
    }
    ui.label("Global default JDK");
    let selection_changed = jdk_selector(
        ui,
        "global_jdk",
        &mut state.settings.default_jdk_id,
        &mut state.settings.jdks,
        "No global default",
        "No global default",
        "Choose JDK…",
        0,
        crate::java::jdk::JdkSelectionTarget::Global,
        &mut state.events,
        state.jdk_discovery.has_worker(),
    );
    changed |= selection_changed;
    if selection_changed {
        state.pending_language_restart = true;
        state.status = "Restarting Java intelligence with the selected default JDK…".into();
    }
    let minimum_jdt_java = state.settings.jdt_ls_minimum_major();
    ui.weak("Changing the selected JDK restarts Java intelligence automatically.");
    ui.label(format!("JDT Language Server runtime (Java {minimum_jdt_java}+)")).on_hover_text(if state.settings.jdt_ls_executable.is_some() { "A custom JDT LS launcher requires Java 21 or newer." } else { "The bundled JDT LS requires Java 25 or newer." });
    let selection_changed = jdk_selector(
        ui,
        "jdt_ls_jdk",
        &mut state.settings.jdt_ls_jdk_id,
        &mut state.settings.jdks,
        "No JDT LS runtime",
        "No JDT LS runtime",
        &format!("Choose Java {minimum_jdt_java}+…"),
        minimum_jdt_java,
        crate::java::jdk::JdkSelectionTarget::JdtLs,
        &mut state.events,
        state.jdk_discovery.has_worker(),
    );
    if !selection_changed
        && let (Some(selected), Ok(runtime)) = (
            state.settings.jdt_ls_jdk_id.as_deref(),
            state.settings.jdt_ls_jdk(),
        )
        && selected != runtime.id
    {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!(
                "Saved JDT LS runtime is incompatible; using {} for Java intelligence.",
                runtime.label()
            ),
        );
    }
    changed |= selection_changed;
    if selection_changed {
        state.pending_language_restart = true;
        state.status = "Restarting Java intelligence with the selected JDT LS runtime…".into();
    }
    ui.label("JDT LS launcher override (bundled launcher is used when unset)");
    ui.horizontal(|ui| {
        let display = state
            .language
            .tooling
            .jdtls_path
            .as_ref()
            .map(|path| {
                if state.settings.jdt_ls_executable.is_some() {
                    format!("Override: {}", path.display())
                } else {
                    format!("Bundled: {}", path.display())
                }
            })
            .unwrap_or_else(|| {
                if state.language.tooling.resolving {
                    "Checking launcher…".into()
                } else {
                    "No launcher found — editor and builds remain available".into()
                }
            });
        ui.label(display);
        if ui.small_button("Choose launcher…").clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Select installed jdtls launcher")
                .pick_file()
            {
                state.settings.jdt_ls_executable = Some(path);
                changed = true;
            }
        }
        if state.settings.jdt_ls_executable.is_some() && ui.small_button("Clear launcher").clicked()
        {
            state.settings.jdt_ls_executable = None;
            changed = true;
        }
    });
    if let Some(error) = &state.language.tooling.jdtls_error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    ui.label("Java Debug Server plugin override (bundled plugin is used when unset)");
    ui.horizontal(|ui| {
        let display = state
            .language
            .tooling
            .debug_bundle
            .as_ref()
            .map(|path| {
                if state.settings.java_debug_bundle.is_some() {
                    format!("Override: {}", path.display())
                } else {
                    format!("Bundled: {}", path.display())
                }
            })
            .unwrap_or_else(|| {
                if state.language.tooling.resolving {
                    "Checking plugin…".into()
                } else {
                    "No plugin found".into()
                }
            });
        ui.label(display);
        if ui.small_button("Choose plugin JAR…").clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("Java Debug Server plugin", &["jar"])
                .set_title("Select com.microsoft.java.debug.plugin-<version>.jar")
                .pick_file()
            {
                state.settings.java_debug_bundle = Some(path);
                changed = true;
            }
        }
        if state.settings.java_debug_bundle.is_some() && ui.small_button("Clear plugin").clicked() {
            state.settings.java_debug_bundle = None;
            changed = true;
        }
    });
    if let Some(error) = &state.language.tooling.debug_error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    } else if state.language.tooling.debug_bundle.is_some() {
        ui.small("Plugin path is valid. Apply / restart Java intelligence to load it. Compatibility is checked by JDT LS.");
    }
    if ui.button("Apply / restart Java intelligence").clicked() {
        state.enqueue(AppEvent::RestartLanguage);
    }
    ui.small(&state.language.summary);
    ui.separator();
    ui.label("Build executables (projects prefer their wrapper)");
    for (label, executable) in [
        ("Maven", &mut state.settings.maven_executable),
        ("Gradle", &mut state.settings.gradle_executable),
    ] {
        ui.horizontal(|ui| {
            ui.label(label);
            ui.label(
                executable
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "Unconfigured".into()),
            );
            if ui.small_button("Choose…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .set_title(format!("Select {label} executable"))
                    .pick_file()
                {
                    *executable = Some(path);
                    changed = true;
                }
            }
            if executable.is_some() && ui.small_button("Clear").clicked() {
                *executable = None;
                changed = true;
            }
        });
    }
    if changed {
        state.enqueue(AppEvent::SettingsChanged);
    }
}

pub fn project_jdk_ui(ui: &mut egui::Ui, state: &mut AppState) {
    let Some(project) = &state.project else {
        ui.label("Open a project first");
        return;
    };
    let root = project.root.clone();
    let display_name = project.display_name.clone();
    ui.strong(&display_name);
    ui.small(root.display().to_string());
    ui.label("Java installations");
    jdk_catalog_controls(ui, state);
    ui.separator();
    ui.label("Project default JDK");
    let mut selected = state.settings.project_jdk_id(&root).map(str::to_owned);
    let inherited = state
        .settings
        .default_jdk_id
        .as_deref()
        .and_then(|id| state.settings.jdks.iter().find(|jdk| jdk.id == id))
        .map(|jdk| format!("Use global default ({})", jdk.label()))
        .unwrap_or_else(|| "Use global default (unconfigured)".into());
    let selection_changed = jdk_selector(
        ui,
        "project_jdk",
        &mut selected,
        &mut state.settings.jdks,
        &inherited,
        &inherited,
        "Add JDK…",
        0,
        crate::java::jdk::JdkSelectionTarget::Project(root.clone()),
        &mut state.events,
        state.jdk_discovery.has_worker(),
    );
    if selection_changed {
        state.settings.set_project_jdk(root.clone(), selected);
        state.pending_language_restart = true;
        state.status = "Restarting Java intelligence with the selected project JDK…".into();
    }
    if selection_changed {
        state.enqueue(AppEvent::SettingsChanged);
    }
    match state.settings.selected_jdk(&root, None) {
        Ok(jdk) => {
            ui.label(format!("Effective JDK: {}", jdk.label()));
            ui.small(jdk.home.display().to_string());
        }
        Err(error) => {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }
}

pub(crate) fn restart_language(state: &mut AppState) {
    if state.quit_requested {
        return;
    }
    state.pending_rename_request = None;
    state.pending_workspace_edits.clear();
    state.run.cancel();
    state.debugger.cancel();
    if state.debugger.has_workers() {
        state.pending_language_restart = true;
        return;
    }
    state.pending_language_restart = false;
    let model = state
        .project
        .as_ref()
        .and_then(|project| project.model.as_ref());
    let selection_root = state.project.as_ref().map(|project| project.root.as_path());
    state
        .language
        .restart(model, &state.settings, selection_root);
    state.language.sync_documents(&state.editor_tabs);
}
