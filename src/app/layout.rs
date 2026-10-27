use eframe::egui;

use super::actions;
use super::events::{AppEvent, MavenAction};
use super::{ActivePanel, AppState};
use crate::project::ProjectKind;
use crate::project::maven_view::{MavenModuleView, MavenTask};
use crate::project::tree::TreeAction;

pub fn render(ui: &mut egui::Ui, state: &mut AppState) {
    egui::Panel::top("main_menu").show(ui, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New Java Project…").clicked() {
                    state.enqueue(AppEvent::NewJavaProjectOpen);
                    ui.close();
                }
                if ui.button("Open Project…     ⇧⌘O").clicked() {
                    ui.close();
                    actions::open_project_dialog(state);
                }
                if ui.button("Open File…             ⌘O").clicked() {
                    ui.close();
                    actions::open_file_dialog(state);
                }
                if ui.add_enabled(state.project.is_some(), egui::Button::new("Go to File…             ⌘P")).clicked() {
                    state.enqueue(AppEvent::QuickOpenOpen);
                    ui.close();
                }
                ui.menu_button("Recent Projects", |ui| {
                    if state.settings.recent_projects.is_empty() {
                        ui.label("No recent projects");
                    }
                    for path in state.settings.recent_projects.clone() {
                        if ui.button(path.display().to_string()).clicked() {
                            state.enqueue(AppEvent::OpenProject(path));
                            ui.close();
                        }
                    }
                });
                ui.separator();
                let has_document = state.active_tab.is_some();
                if ui
                    .add_enabled(
                        has_document,
                        egui::Button::new("Save                        ⌘S"),
                    )
                    .clicked()
                {
                    state.enqueue(AppEvent::SaveActiveFile);
                    ui.close();
                }
                if ui
                    .add_enabled(
                        !state.editor_tabs.is_empty(),
                        egui::Button::new("Save All                 ⇧⌘S"),
                    )
                    .clicked()
                {
                    state.enqueue(AppEvent::SaveAllFiles);
                    ui.close();
                }
                if ui
                    .add_enabled(
                        has_document,
                        egui::Button::new("Close File                ⌘W"),
                    )
                    .clicked()
                {
                    state.enqueue(AppEvent::CloseActiveFile);
                    ui.close();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    state.enqueue(AppEvent::RequestQuit);
                    ui.close();
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui.add_enabled(state.active_tab.is_some(), egui::Button::new("Rename Symbol…    Shift+F6")).clicked() {
                    state.enqueue(AppEvent::RenameSymbolOpen);
                    ui.close();
                }
                ui.separator();
                if ui.button("Find in Project…    ⇧⌘F").clicked() {
                    state.enqueue(AppEvent::Search(super::events::SearchAction::Open));
                    ui.close();
                }
                if ui.button("Find in File…    ⌘F").clicked() {
                    state.enqueue(AppEvent::Find(crate::editor::find_replace::FindAction::Open { replace: false }));
                    ui.close();
                }
                if ui.button("Replace in File…    ⌘R").clicked() {
                    state.enqueue(AppEvent::Find(crate::editor::find_replace::FindAction::Open { replace: true }));
                    ui.close();
                }
                if ui.add_enabled(state.active_tab.is_some(), egui::Button::new("Go to Line…    ⌘L")).clicked() {
                    state.enqueue(AppEvent::GoToLineOpen);
                    ui.close();
                }
            });
            ui.menu_button("Settings", |ui| {
                actions::jdk_settings_ui(ui, state);
            });
            ui.menu_button("Project Structure", |ui| {
                actions::project_jdk_ui(ui, state);
            });
            ui.menu_button("Code", |ui| {
                for (label, command) in [("Complete    Ctrl+Space", crate::java::lsp::EditorCommand::Completion),
                    ("Parameter info    Ctrl+Shift+Space", crate::java::lsp::EditorCommand::SignatureHelp),
                    ("Hover    F1", crate::java::lsp::EditorCommand::Hover),
                    ("Go to definition    F12", crate::java::lsp::EditorCommand::Definition),
                    ("Go to type definition    ⇧⌘B", crate::java::lsp::EditorCommand::TypeDefinition),
                    ("Go to implementation    ⌥⌘B", crate::java::lsp::EditorCommand::Implementation),
                    ("Find references    Shift+F12", crate::java::lsp::EditorCommand::References),
                    ("Organize imports    Ctrl+Alt+O", crate::java::lsp::EditorCommand::OrganizeImports),
                    ("Reformat code    ⌥⌘L", crate::java::lsp::EditorCommand::Format)] {
                    if ui.add_enabled(state.active_tab.is_some(), egui::Button::new(label)).clicked() {
                        state.enqueue(AppEvent::EditorCommand(command)); ui.close();
                    }
                }
                if ui.button("Go to Java Symbol…    ⌘⌥O").clicked() {
                    state.enqueue(AppEvent::SymbolSearchOpen);
                    ui.close();
                }
                if ui.add_enabled(state.active_tab.is_some(), egui::Button::new("File Structure…    ⌘F12")).clicked() {
                    state.enqueue(AppEvent::FileStructureOpen);
                    ui.close();
                }
            });
            ui.menu_button("Run", |ui| {
                let can_launch = state.project.as_ref()
                    .and_then(|project| project.model.as_ref())
                    .is_some_and(|model| model.kind != ProjectKind::Unknown)
                    && state.run.selected().is_some()
                    && !state.run.loading
                    && !state.run.saving;
                if ui.add_enabled(can_launch && !state.run.has_workers(), egui::Button::new("Run Selected Configuration")).clicked() {
                    state.enqueue(AppEvent::Run(super::events::RunAction::Start));
                    ui.close();
                }
                if ui.add_enabled(can_launch && !state.debugger.has_workers(), egui::Button::new("Debug Selected Configuration")).clicked() {
                    state.enqueue(AppEvent::Debugger(super::events::DebuggerAction::Start));
                    ui.close();
                }
                if ui.button("Run Configurations…").clicked() {
                    state.enqueue(AppEvent::ShowPanel(ActivePanel::Run));
                    ui.close();
                }
                ui.separator();
                if ui.add_enabled(state.run.has_workers(), egui::Button::new("Stop Run")).clicked() {
                    state.enqueue(AppEvent::Run(super::events::RunAction::Stop));
                    ui.close();
                }
                if ui.add_enabled(state.debugger.has_workers(), egui::Button::new("Stop Debug Session")).clicked() {
                    state.enqueue(AppEvent::Debugger(super::events::DebuggerAction::Stop));
                    ui.close();
                }
            });
            ui.menu_button("View", |ui| {
                let changed = ui
                    .checkbox(&mut state.settings.show_project_panel, "Project sidebar")
                    .changed()
                    | ui.checkbox(&mut state.settings.show_tool_window, "Tool window")
                        .changed();
                if changed {
                    state.enqueue(AppEvent::SettingsChanged);
                }
                if state.maven_view_root.is_some()
                    && ui.button(if state.maven_collapsed { "Show Maven sidebar" } else { "Hide Maven sidebar" }).clicked() {
                    state.enqueue(AppEvent::Maven(MavenAction::ToggleCollapsed));
                }
                if state.gradle_task_root.is_some()
                    && ui.button(if state.gradle_collapsed { "Show Gradle sidebar" } else { "Hide Gradle sidebar" }).clicked() {
                    state.enqueue(AppEvent::Gradle(super::events::GradleAction::ToggleCollapsed));
                }
                if ui
                    .checkbox(&mut state.settings.dark_mode, "Dark appearance")
                    .changed()
                {
                    ui.ctx().set_theme(if state.settings.dark_mode {
                        egui::Theme::Dark
                    } else {
                        egui::Theme::Light
                    });
                    state.enqueue(AppEvent::SettingsChanged);
                }
            });
            ui.separator();
            ui.strong("Intelli-gen");
            ui.label("Java Workbench");
        });
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Open Project").clicked() {
                actions::open_project_dialog(state);
            }
            if ui.button("Open File").clicked() {
                actions::open_file_dialog(state);
            }
            ui.separator();
            if ui.add_enabled(state.project.as_ref().and_then(|project| project.model.as_ref()).is_some() && !state.build.running(), egui::Button::new("Build")).clicked() {
                state.enqueue(AppEvent::Build(super::events::BuildAction::Build));
            }
            crate::java::run_config::configuration_selector(ui, &mut state.run);
            if ui.add_enabled(state.run.selected().is_some() && !state.run.has_workers(), egui::Button::new("Run")).clicked() {
                state.enqueue(AppEvent::Run(super::events::RunAction::Start));
            }
            if ui.button("Configurations…").clicked() { state.enqueue(AppEvent::ShowPanel(ActivePanel::Run)); }
            let debug_enabled = state.run.selected().is_some() && !state.debugger.has_workers()
                && state.language.tooling.debug_bundle.is_some() && !state.pending_language_restart;
            if ui.add_enabled(debug_enabled, egui::Button::new("Debug"))
                .on_disabled_hover_text("Install the bundled Java Debug Server or choose a valid plugin JAR override in Settings, and select a run configuration").clicked() {
                state.enqueue(AppEvent::Debugger(super::events::DebuggerAction::Start));
            }
            if let Some(project) = &state.project {
                ui.separator();
                ui.label(&project.display_name)
                    .on_hover_text(project.root.display().to_string());
            }
        });
    });

    egui::Panel::bottom("status_bar").show(ui, |ui| {
        if let Some(tab) = state
            .active_tab
            .and_then(|index| state.editor_tabs.get(index))
        {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "Ln {}, Col {}",
                    tab.document.cursor_line, tab.document.cursor_column
                ));
                ui.separator();
                ui.label(tab.path.display().to_string());
            });
        }
        if state.project_opening {
            ui.spinner();
        }
        if state.settings_save_pending {
            ui.spinner();
            ui.label("Saving preferences…");
        }
        ui.label(&state.status);
        for error in [&state.settings_load_error, &state.settings_save_error]
            .into_iter()
            .flatten()
        {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if let Some(error) = &state.last_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    });

    // Tool tabs remain reachable even while their content is hidden.
    egui::Panel::bottom("tool_tabs").show(ui, |ui| {
        ui.horizontal(|ui| {
            for panel in ActivePanel::ALL {
                if ui
                    .selectable_label(
                        state.settings.show_tool_window && state.active_panel == panel,
                        panel.title(),
                    )
                    .clicked()
                {
                    state.enqueue(AppEvent::ShowPanel(panel));
                }
            }
            if state.settings.show_tool_window && ui.small_button("Hide").clicked() {
                state.settings.show_tool_window = false;
                state.enqueue(AppEvent::SettingsChanged);
            }
        });
    });

    if state.settings.show_project_panel {
        egui::Panel::left("project_sidebar")
            .resizable(true)
            .default_size(250.0)
            .size_range(160.0..=480.0)
            .show(ui, |ui| {
                ui.heading("Project");
                ui.separator();
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if let Some(project) = &state.project {
                            let root = project.root.clone();
                            let response = ui
                                .strong(&project.display_name)
                                .on_hover_text(root.display().to_string());
                            response.context_menu(|ui| {
                                if ui.button("New Java Class…").clicked() {
                                    state.enqueue(AppEvent::NewJavaClass(root.clone()));
                                    ui.close();
                                }
                                if ui.button("Refresh").clicked() {
                                    state.enqueue(AppEvent::RefreshProjectTree(root.clone()));
                                    ui.close();
                                }
                                if ui.button("Reveal in Finder").clicked() {
                                    state.enqueue(AppEvent::RevealInFinder(root.clone()));
                                    ui.close();
                                }
                            });
                            if ui
                                .add_enabled(
                                    !state.project_tree_loading,
                                    egui::Button::new("Refresh"),
                                )
                                .clicked()
                            {
                                state.enqueue(AppEvent::RefreshProjectTree(root));
                            }
                            if state.project_tree_loading {
                                ui.spinner();
                            }
                            if let Some(error) = &state.project_tree_error {
                                ui.colored_label(ui.visuals().error_fg_color, error);
                            }
                            if let Some(tree) = &mut state.project_tree {
                                let actions = tree.render(ui);
                                for action in actions {
                                    state.enqueue(match action {
                                        TreeAction::OpenFile(path) => AppEvent::OpenFile(path),
                                        TreeAction::NewJavaClass(path) => AppEvent::NewJavaClass(path),
                                        TreeAction::Refresh(path) => {
                                            AppEvent::RefreshProjectTree(path)
                                        }
                                        TreeAction::Reveal(path) => AppEvent::RevealInFinder(path),
                                        TreeAction::LoadChildren { path, request } => {
                                            AppEvent::LoadProjectTreeChildren { path, request }
                                        }
                                        TreeAction::ValidateRoot { request } => {
                                            AppEvent::ValidateProjectTreeRoot { request }
                                        }
                                    });
                                }
                            }
                        } else {
                            ui.label("No project open");
                            if ui.button("Choose project directory…").clicked() {
                                actions::open_project_dialog(state);
                            }
                        }
                    });
            });
    }

    if state
        .project
        .as_ref()
        .and_then(|project| project.model.as_ref())
        .is_some_and(|model| model.kind == ProjectKind::Maven)
    {
        if state.maven_collapsed {
            egui::Panel::right("maven_sidebar_collapsed")
                .resizable(false)
                .default_size(32.0)
                .size_range(32.0..=32.0)
                .show(ui, |ui| {
                    if ui
                        .small_button("◀")
                        .on_hover_text("Show Maven sidebar")
                        .clicked()
                    {
                        state.enqueue(AppEvent::Maven(MavenAction::ToggleCollapsed));
                    }
                    ui.label("M").on_hover_text("Maven");
                });
        } else {
            egui::Panel::right("maven_sidebar")
                .resizable(true)
                .default_size(300.0)
                .size_range(220.0..=600.0)
                .show(ui, |ui| render_maven_sidebar(ui, state));
        }
    }

    if state.gradle_task_root.is_some() {
        if state.gradle_collapsed {
            egui::Panel::right("gradle_sidebar_collapsed")
                .resizable(false)
                .default_size(32.0)
                .size_range(32.0..=32.0)
                .show(ui, |ui| {
                    if ui.small_button("◀").on_hover_text("Show Gradle sidebar").clicked() {
                        state.enqueue(AppEvent::Gradle(super::events::GradleAction::ToggleCollapsed));
                    }
                    ui.label("G").on_hover_text("Gradle");
                });
        } else {
            egui::Panel::right("gradle_sidebar")
                .resizable(true)
                .default_size(300.0)
                .size_range(220.0..=600.0)
                .show(ui, |ui| render_gradle_sidebar(ui, state));
        }
    }

    if state.settings.show_tool_window {
        egui::Panel::bottom("tool_content")
            .resizable(true)
            .default_size(200.0)
            .size_range(90.0..=450.0)
            .show(ui, |ui| {
                ui.strong(state.active_panel.title());
                ui.separator();
                match state.active_panel {
                    ActivePanel::Search => crate::search::render(ui, state),
                    ActivePanel::Terminal => crate::terminal::render(ui, state),
                    ActivePanel::Git => crate::git::render(ui, state),
                    ActivePanel::Build => crate::java::build::render(ui, state),
                    ActivePanel::Run => crate::java::run_config::render(ui, state),
                    ActivePanel::Debugger => crate::debugger::render(ui, state),
                }
            });
    }

    egui::CentralPanel::default().show(ui, |ui| {
        if state.editor_tabs.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(60.0);
                ui.heading("Java Workbench");
                ui.label("Open a Maven or Gradle project, then select a source file.");
                ui.add_space(12.0);
                if ui.button("Open Project…").clicked() {
                    actions::open_project_dialog(state);
                }
                if ui.button("Open File…").clicked() {
                    actions::open_file_dialog(state);
                }
            });
        } else {
            crate::editor::tabs::render(ui, state);
        }
    });
    if let Some(path) = state.quick_open.render(ui.ctx()) {
        state.enqueue(AppEvent::OpenFile(path));
    }
    let (symbol_actions, symbol_location) = state.symbol_search.render(ui.ctx(), &state.language);
    for action in symbol_actions {
        state.enqueue(action);
    }
    if let Some(location) = symbol_location {
        state.enqueue(AppEvent::OpenFileAtPosition(
            location.path,
            location.range.start,
        ));
    }
    if let Some(location) = state.file_structure.render(ui.ctx(), &state.language) {
        state.enqueue(AppEvent::OpenFileAtPosition(
            location.path,
            location.range.start,
        ));
    }
    if let Some(line) = state.go_to_line.render(ui.ctx()) {
        if let Some(tab) = state.active_tab.and_then(|index| state.editor_tabs.get(index)) {
            state.enqueue(AppEvent::OpenFileAtLine(tab.path.clone(), line));
        }
    }
    for action in state.find_replace.render(ui.ctx()) {
        state.enqueue(AppEvent::Find(action));
    }
    if let Some(dialog) = state.new_java_class.as_mut() {
        let mut create = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::unique("new_java_class_modal")).show(ui.ctx(), |ui| {
            ui.heading("New Java Class");
            ui.label(dialog.directory.display().to_string());
            ui.add(egui::TextEdit::singleline(&mut dialog.name).hint_text("Class name, e.g. CustomerService"));
            ui.add(egui::TextEdit::singleline(&mut dialog.package).hint_text("Package (optional; creates matching folders, e.g. com.example.service)"));
            ui.horizontal(|ui| {
                create = ui.add_enabled(!dialog.name.trim().is_empty(), egui::Button::new("Create")).clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        if create {
            if let Some(dialog) = state.new_java_class.take() {
                state.enqueue(AppEvent::CreateJavaClass {
                    directory: dialog.directory,
                    name: dialog.name.trim().to_owned(),
                    package: dialog.package.trim().to_owned(),
                });
            }
        } else if cancel {
            state.new_java_class = None;
        }
    }
    if let Some(dialog) = state.new_java_project.as_mut() {
        let mut create = false;
        let mut cancel = false;
        let mut choose_parent = false;
        egui::Modal::new(egui::Id::unique("new_java_project_modal")).show(ui.ctx(), |ui| {
            ui.heading("New Java Project");
            ui.horizontal(|ui| {
                ui.label("Location");
                ui.label(dialog.parent.display().to_string());
                choose_parent = ui.button("Choose…").clicked();
            });
            ui.add(egui::TextEdit::singleline(&mut dialog.name).hint_text("Project name"));
            ui.add(egui::TextEdit::singleline(&mut dialog.package).hint_text("Java package (optional, e.g. com.example)"));
            egui::ComboBox::from_id_salt("new_java_project_kind")
                .selected_text(dialog.kind.to_string())
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut dialog.kind, ProjectKind::Maven, "Maven");
                    ui.selectable_value(&mut dialog.kind, ProjectKind::Gradle, "Gradle");
                });
            ui.horizontal(|ui| {
                create = ui.add_enabled(!dialog.name.trim().is_empty(), egui::Button::new("Create")).clicked();
                cancel = ui.button("Cancel").clicked();
            });
            ui.small("Creates a Java 8 starter project locally. No tools or dependencies are downloaded.");
        });
        if choose_parent {
            if let Some(parent) = rfd::FileDialog::new()
                .set_title("Choose project parent folder")
                .set_directory(&dialog.parent)
                .pick_folder()
            {
                dialog.parent = parent;
            }
        }
        if create {
            if let Some(dialog) = state.new_java_project.take() {
                state.enqueue(AppEvent::CreateJavaProject {
                    parent: dialog.parent,
                    name: dialog.name.trim().to_owned(),
                    package: dialog.package.trim().to_owned(),
                    kind: dialog.kind,
                });
            }
        } else if cancel {
            state.new_java_project = None;
        }
    }
    if let Some(dialog) = state.rename_symbol.as_mut() {
        let mut confirm = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::unique("rename_symbol_modal")).show(ui.ctx(), |ui| {
            ui.heading("Rename Java Symbol");
            ui.label(format!("{} in {}", dialog.old_name, dialog.path.display()));
            let input_id = egui::Id::new("rename_symbol_name");
            let was_focused = ui.ctx().memory(|memory| memory.has_focus(input_id));
            let input = ui.add(egui::TextEdit::singleline(&mut dialog.new_name).id(input_id).hint_text("New name"));
            if !was_focused { input.request_focus(); }
            confirm = ui.add_enabled(!dialog.new_name.trim().is_empty(), egui::Button::new("Rename")).clicked()
                || input.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            cancel = ui.button("Cancel").clicked();
        });
        if confirm {
            let new_name = dialog.new_name.trim().to_owned();
            state.rename_symbol = None;
            state.enqueue(AppEvent::RenameSymbolConfirm(new_name));
        } else if cancel {
            state.rename_symbol = None;
        }
    }
}

fn render_maven_sidebar(ui: &mut egui::Ui, state: &mut AppState) {
    let mut actions = Vec::new();
    ui.horizontal(|ui| {
        ui.heading("Maven");
        if let Some(root) = &state.maven_view_root {
            if ui
                .small_button("↻")
                .on_hover_text("Reload Maven structure")
                .clicked()
            {
                actions.push(AppEvent::Maven(MavenAction::Reload { root: root.clone() }));
            }
        }
        if ui
            .small_button("▶")
            .on_hover_text("Hide Maven sidebar")
            .clicked()
        {
            actions.push(AppEvent::Maven(MavenAction::ToggleCollapsed));
        }
    });
    ui.separator();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if state.maven_loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Loading effective Maven model…");
                });
            } else if state.maven_cache_loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Loading cached Maven model…");
                });
            }
            if let Some(error) = &state.maven_error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            if let Some(error) = &state.maven_cache_error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            let Some(root) = state.maven_view_root.as_ref() else {
                return;
            };
            if let Some(view) = &state.maven_view {
                egui::CollapsingHeader::new("Profiles")
                    .default_open(true)
                    .show(ui, |ui| {
                        for profile in &view.profiles {
                            let selected = state.maven_profiles.contains(&profile.id);
                            let label = if profile.active_by_default {
                                format!("{} (default)", profile.id)
                            } else {
                                profile.id.clone()
                            };
                            if profile.id == "local" {
                                let mut checked = true;
                                ui.add_enabled(false, egui::Checkbox::new(&mut checked, label))
                                    .on_disabled_hover_text(
                                        "The local profile is always active when defined",
                                    );
                            } else {
                                let mut checked = selected;
                                if ui.checkbox(&mut checked, label).changed() {
                                    actions.push(AppEvent::Maven(MavenAction::ToggleProfile {
                                        root: root.clone(),
                                        profile: profile.id.clone(),
                                    }));
                                }
                            }
                        }
                    });
                ui.separator();
                let enabled = !state.maven_loading && state.maven_error.is_none();
                render_maven_module(
                    ui,
                    &view.root_module,
                    root,
                    state.maven_generation,
                    state.maven_selected_module.as_ref(),
                    enabled,
                    true,
                    &mut actions,
                );
            } else if !state.maven_loading
                && !state.maven_cache_loading
                && state.maven_error.is_none()
            {
                ui.label("No cached Maven model. Select ↻ to load it.");
            }
        });
    for action in actions {
        state.enqueue(action);
    }
}

fn render_gradle_sidebar(ui: &mut egui::Ui, state: &mut AppState) {
    use super::events::GradleAction;
    let mut actions = Vec::new();
    ui.horizontal(|ui| {
        ui.heading("Gradle");
        if let Some(root) = &state.gradle_task_root {
            let (label, hint, action) = if state.gradle_task_loading {
                ("Cancel", "Cancel Gradle task discovery", GradleAction::Cancel { root: root.clone() })
            } else if state.gradle_task_loaded {
                ("↻", "Reload Gradle tasks", GradleAction::Reload { root: root.clone() })
            } else {
                ("Load tasks", "Run Gradle only after this explicit action", GradleAction::Reload { root: root.clone() })
            };
            if ui.small_button(label).on_hover_text(hint).clicked() {
                actions.push(AppEvent::Gradle(action));
            }
        }
        if ui.small_button("▶").on_hover_text("Hide Gradle sidebar").clicked() {
            actions.push(AppEvent::Gradle(GradleAction::ToggleCollapsed));
        }
    });
    ui.separator();
    let Some(root) = state.gradle_task_root.clone() else { return; };
    let Some(model) = state.project.as_ref().and_then(|project| project.model.as_ref()) else { return; };
    let project_name = state.gradle_selected_project.as_ref()
        .and_then(|path| model.modules.iter().find(|module| &module.root == path).map(|module| module.name.clone()))
        .unwrap_or_else(|| format!("{} (root)", model.name));
    let mut selected = state.gradle_selected_project.clone();
    egui::ComboBox::from_id_salt("gradle_project_selector")
        .selected_text(project_name)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut selected, None, format!("{} (root)", model.name));
            for module in &model.modules {
                ui.selectable_value(&mut selected, Some(module.root.clone()), &module.name);
            }
        });
    if selected != state.gradle_selected_project {
        actions.push(AppEvent::Gradle(GradleAction::SelectProject { root: root.clone(), project_path: selected }));
    }
    ui.horizontal(|ui| {
        ui.label("Filter");
        ui.text_edit_singleline(&mut state.gradle_task_filter);
    });
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if state.gradle_task_loading {
            ui.horizontal(|ui| { ui.spinner(); ui.label("Loading Gradle tasks…"); });
        }
        if !state.gradle_task_loaded && !state.gradle_task_loading {
            ui.weak("Load tasks to inspect this Gradle project.");
        }
        if let Some(error) = &state.gradle_task_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        let filter = state.gradle_task_filter.trim().to_lowercase();
        let enabled = !state.gradle_task_loading && state.gradle_task_error.is_none() && !state.build.running();
        let generation = state.gradle_task_generation;
        let module = state.gradle_selected_project.clone();
        let mut count = 0;
        for task in &state.gradle_tasks {
            if !filter.is_empty() && !task.path.to_lowercase().contains(&filter)
                && !task.description.to_lowercase().contains(&filter)
            {
                continue;
            }
            count += 1;
            if ui.add_enabled(enabled, egui::Button::new(egui::RichText::new(&task.path).monospace()))
                .on_hover_text(&task.description).clicked()
            {
                actions.push(AppEvent::Gradle(GradleAction::RunTask {
                    root: root.clone(), generation, project_path: module.clone(), task_path: task.path.clone(),
                }));
            }
            if !task.description.is_empty() { ui.weak(&task.description); }
        }
        if count == 0 && state.gradle_task_loaded && !state.gradle_task_loading && state.gradle_task_error.is_none() {
            ui.weak("No matching Gradle tasks");
        }
    });
    for action in actions { state.enqueue(action); }
}

fn render_maven_module(
    ui: &mut egui::Ui,
    module: &MavenModuleView,
    root: &std::path::Path,
    generation: u64,
    selected: Option<&std::path::PathBuf>,
    enabled: bool,
    is_root: bool,
    actions: &mut Vec<AppEvent>,
) {
    let is_selected = module.relative_path.as_deref() == selected.map(std::path::PathBuf::as_path);
    let title = if is_selected {
        format!("● {}", module.name)
    } else {
        module.name.clone()
    };
    let response = egui::CollapsingHeader::new(title)
        .id_salt(("maven_module", module.relative_path.as_ref()))
        .default_open(is_root)
        .show(ui, |ui| {
            let module_path = module.relative_path.clone();
            egui::CollapsingHeader::new("Lifecycle")
                .id_salt("lifecycle")
                .show(ui, |ui| {
                    for task in &module.lifecycle {
                        render_maven_task(
                            ui,
                            task,
                            root,
                            generation,
                            &module_path,
                            enabled,
                            actions,
                        );
                    }
                });
            egui::CollapsingHeader::new("Plugins")
                .id_salt("plugins")
                .show(ui, |ui| {
                    for plugin in &module.plugins {
                        egui::CollapsingHeader::new(&plugin.name)
                            .id_salt(&plugin.name)
                            .show(ui, |ui| {
                                for task in &plugin.tasks {
                                    render_maven_task(
                                        ui,
                                        task,
                                        root,
                                        generation,
                                        &module_path,
                                        enabled,
                                        actions,
                                    );
                                }
                            });
                    }
                });
            egui::CollapsingHeader::new("Dependencies")
                .id_salt("dependencies")
                .show(ui, |ui| {
                    for dependency in &module.dependencies {
                        ui.label(dependency);
                    }
                });
            if is_root {
                egui::CollapsingHeader::new("Repositories")
                    .id_salt("repositories")
                    .show(ui, |ui| {
                        for repository in &module.repositories {
                            ui.label(repository);
                        }
                    });
            }
            for child in &module.modules {
                render_maven_module(
                    ui, child, root, generation, selected, enabled, false, actions,
                );
            }
        });
    if response.header_response.clicked() {
        actions.push(AppEvent::Maven(MavenAction::SelectModule {
            root: root.to_path_buf(),
            module: module.relative_path.clone(),
        }));
    }
}

fn render_maven_task(
    ui: &mut egui::Ui,
    task: &MavenTask,
    root: &std::path::Path,
    generation: u64,
    module: &Option<std::path::PathBuf>,
    enabled: bool,
    actions: &mut Vec<AppEvent>,
) {
    if ui
        .add_enabled(enabled, egui::Button::new(&task.label))
        .on_hover_text(&task.goal)
        .clicked()
    {
        actions.push(AppEvent::Maven(MavenAction::RunTask {
            root: root.to_path_buf(),
            generation,
            module: module.clone(),
            goal: task.goal.clone(),
        }));
    }
}
