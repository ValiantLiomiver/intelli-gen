use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use eframe::egui;

use super::ProjectError;

pub(crate) const EXCLUDED: [&str; 4] = [".git", "target", "build", ".gradle"];
static NEXT_TREE_REQUEST: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub struct ProjectTree {
    pub root: PathBuf,
    entries: Vec<TreeEntry>,
    focused_path: Option<PathBuf>,
    focus_pending: bool,
    typeahead_query: String,
    typeahead_at: Option<Instant>,
    root_error: Option<String>,
    root_loading_request: Option<u64>,
    root_validation_request: Option<u64>,
    next_root_validation: Instant,
}

#[derive(Clone, Debug)]
struct TreeEntry {
    path: PathBuf,
    name: String,
    directory: bool,
    symlink: bool,
    children: Option<Vec<TreeEntry>>,
    loading_request: Option<u64>,
    error: Option<String>,
}

pub enum TreeAction {
    OpenFile(PathBuf),
    NewJavaClass(PathBuf),
    Refresh(PathBuf),
    Reveal(PathBuf),
    LoadChildren { path: PathBuf, request: u64 },
    ValidateRoot { request: u64 },
}

#[derive(Clone, Debug)]
pub struct ProjectTreeChildren {
    pub parent: PathBuf,
    pub request: u64,
    entries: Vec<TreeEntry>,
}

impl ProjectTree {
    /// Read one directory only except when Java package labels need a single-child chain.
    pub fn read(root: &Path) -> Result<Self, ProjectError> {
        // Anchor the requested location without following a replaced root link.
        // In particular, a refresh must not silently adopt a new workspace.
        let root = std::path::absolute(root).map_err(|source| ProjectError {
            path: root.to_path_buf(),
            source,
        })?;
        Ok(Self {
            entries: read_visible_entries(&root, &root)?,
            root,
            focused_path: None,
            focus_pending: false,
            typeahead_query: String::new(),
            typeahead_at: None,
            root_error: None,
            root_loading_request: None,
            root_validation_request: None,
            next_root_validation: Instant::now() + Duration::from_secs(1),
        })
    }

    pub fn begin_refresh(&mut self, path: &Path) -> Option<u64> {
        if path == self.root {
            let request = NEXT_TREE_REQUEST.fetch_add(1, Ordering::Relaxed);
            self.root_loading_request = Some(request);
            self.root_error = None;
            return Some(request);
        }
        let entry = find_entry_mut(&mut self.entries, path)?;
        if !entry.directory || entry.symlink {
            return None;
        }
        let request = NEXT_TREE_REQUEST.fetch_add(1, Ordering::Relaxed);
        entry.loading_request = Some(request);
        entry.error = None;
        Some(request)
    }

    pub fn read_children(
        workspace: &Path,
        path: &Path,
        request: u64,
    ) -> Result<ProjectTreeChildren, String> {
        read_visible_entries(workspace, path)
            .map(|entries| ProjectTreeChildren {
                parent: path.to_path_buf(),
                request,
                entries,
            })
            .map_err(|error| error.to_string())
    }

    pub fn validate_root(root: &Path) -> Result<(), String> {
        checked_directory(root, root)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub fn apply_root_validation(&mut self, request: u64, result: Result<(), String>) {
        if self.root_validation_request != Some(request) {
            return;
        }
        self.root_validation_request = None;
        self.root_error = result.err();
        self.next_root_validation = Instant::now() + Duration::from_secs(1);
    }

    pub fn root_validation_delay(&self) -> Option<Duration> {
        self.root_validation_request.is_none().then(|| {
            self.next_root_validation
                .saturating_duration_since(Instant::now())
        })
    }

    pub fn apply_children(
        &mut self,
        parent: &Path,
        request: u64,
        result: Result<ProjectTreeChildren, String>,
    ) {
        if parent == self.root {
            if self.root_loading_request != Some(request) {
                return;
            }
            self.root_loading_request = None;
            match result {
                Ok(children) if children.parent == parent && children.request == request => {
                    self.root_error = None;
                    self.entries =
                        merge_entries(std::mem::take(&mut self.entries), children.entries);
                }
                Ok(_) => {
                    self.root_error = Some("Project-tree response did not match its request".into())
                }
                Err(error) => self.root_error = Some(error),
            }
            return;
        }
        let Some(entry) = find_entry_mut(&mut self.entries, parent) else {
            return;
        };
        if entry.loading_request != Some(request) {
            return;
        }
        entry.loading_request = None;
        match result {
            Ok(children) if children.parent == parent && children.request == request => {
                entry.error = None;
                entry.children = Some(children.entries);
            }
            Ok(_) => entry.error = Some("Project-tree response did not match its request".into()),
            Err(error) => entry.error = Some(error),
        }
    }

    /// Select root or already-expanded directory listings affected by watcher paths.
    pub fn begin_watched_refreshes(&mut self, paths: &[PathBuf]) -> Vec<(PathBuf, u64)> {
        let mut targets = Vec::new();
        for changed in paths {
            let parent = changed.parent().unwrap_or(&self.root);
            let mut candidate = parent;
            let mut found = None;
            while candidate.starts_with(&self.root) {
                if candidate == self.root {
                    found = Some(self.root.clone());
                    break;
                }
                if find_entry(&self.entries, candidate)
                    .is_some_and(|entry| entry.children.is_some())
                {
                    found = Some(candidate.to_path_buf());
                    break;
                }
                let Some(next) = candidate.parent() else {
                    break;
                };
                candidate = next;
            }
            if let Some(path) = found
                && !targets.iter().any(|(target, _)| target == &path)
            {
                if let Some(request) = self.begin_refresh(&path) {
                    targets.push((path, request));
                }
            }
        }
        targets
    }

    pub fn begin_refresh_all_expanded(&mut self) -> Vec<(PathBuf, u64)> {
        let mut paths = vec![self.root.clone()];
        collect_expanded_paths(&self.entries, &mut paths);
        paths
            .into_iter()
            .filter_map(|path| self.begin_refresh(&path).map(|request| (path, request)))
            .collect()
    }

    pub fn render(&mut self, ui: &mut egui::Ui, focus_requested: &mut bool) -> Vec<TreeAction> {
        let mut actions = Vec::new();
        if Instant::now() >= self.next_root_validation && self.root_validation_request.is_none() {
            let request = NEXT_TREE_REQUEST.fetch_add(1, Ordering::Relaxed);
            self.root_validation_request = Some(request);
            actions.push(TreeAction::ValidateRoot { request });
        }
        if self.entries.is_empty() {
            ui.weak(crate::i18n::ui_text(ui, "Empty directory"));
        }
        if let Some(error) = &self.root_error {
            ui.colored_label(
                ui.visuals().error_fg_color,
                crate::i18n::message(ui.ctx(), error),
            );
            return actions;
        }
        if *focus_requested
            && let Some(first) = self
                .entries
                .iter()
                .find(|entry| entry.directory || entry.error.is_none())
        {
            self.focused_path = Some(first.path.clone());
            self.focus_pending = true;
            *focus_requested = false;
        }
        let mut visible = Vec::new();
        let mut has_focus = false;
        render_entries(
            ui,
            &self.root,
            &mut self.entries,
            &mut actions,
            &mut self.focused_path,
            &mut self.focus_pending,
            &mut visible,
            &mut has_focus,
        );
        if self
            .focused_path
            .as_ref()
            .is_some_and(|path| !visible.iter().any(|entry| &entry.path == path))
        {
            self.focused_path = self
                .focused_path
                .as_ref()
                .and_then(|path| {
                    path.ancestors()
                        .find(|ancestor| visible.iter().any(|entry| &entry.path == *ancestor))
                        .map(Path::to_path_buf)
                })
                .or_else(|| visible.first().map(|entry| entry.path.clone()));
            self.focus_pending = self.focused_path.is_some();
            ui.ctx().request_repaint();
        }
        if has_focus {
            let index = self
                .focused_path
                .as_ref()
                .and_then(|path| visible.iter().position(|entry| &entry.path == path));
            let typed = ui.input(|input| {
                if input.modifiers.command || input.modifiers.ctrl || input.modifiers.alt {
                    return String::new();
                }
                input
                    .events
                    .iter()
                    .filter_map(|event| match event {
                        egui::Event::Text(text) => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<String>()
            });
            if !typed.is_empty() && !visible.is_empty() {
                // shortcut: searches visible entries with a one-second prefix; add indexed tree search when collapsed paths must match.
                let continuing = self
                    .typeahead_at
                    .is_some_and(|last| last.elapsed() <= Duration::from_secs(1));
                if !continuing {
                    self.typeahead_query.clear();
                }
                self.typeahead_query.push_str(&typed);
                self.typeahead_at = Some(Instant::now());
                let query = self.typeahead_query.to_lowercase();
                let start = if continuing {
                    index.unwrap_or(0)
                } else {
                    index.map_or(0, |index| (index + 1) % visible.len())
                };
                if let Some(next) = (0..visible.len())
                    .map(|offset| (start + offset) % visible.len())
                    .find(|index| visible[*index].name.to_lowercase().starts_with(&query))
                {
                    self.focused_path = Some(visible[next].path.clone());
                    self.focus_pending = true;
                    ui.ctx().request_repaint();
                }
            }
            let move_to = if ui
                .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown))
            {
                index.map(|index| (index + 1).min(visible.len().saturating_sub(1)))
            } else if ui
                .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp))
            {
                index.map(|index| index.saturating_sub(1))
            } else {
                None
            };
            if let Some(next) = move_to
                && let Some(entry) = visible.get(next)
            {
                self.focused_path = Some(entry.path.clone());
                self.focus_pending = true;
                ui.ctx().request_repaint();
            } else if ui
                .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
                && let Some(entry) = index.and_then(|index| visible.get(index))
            {
                if let Some(id) = entry.collapsing_id {
                    let mut state =
                        egui::collapsing_header::CollapsingState::load_with_default_open(
                            ui.ctx(),
                            id,
                            false,
                        );
                    state.set_open(!state.is_open());
                    state.store(ui.ctx());
                    ui.ctx().request_repaint();
                } else {
                    actions.push(TreeAction::OpenFile(entry.path.clone()));
                }
            } else if ui
                .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight))
                && let Some(entry) = index.and_then(|index| visible.get(index))
                && let Some(id) = entry.collapsing_id
            {
                let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    id,
                    false,
                );
                if state.is_open() {
                    if let Some(child) = visible.get(index.unwrap_or_default() + 1)
                        && child.path.starts_with(&entry.path)
                    {
                        self.focused_path = Some(child.path.clone());
                        self.focus_pending = true;
                    }
                } else {
                    state.set_open(true);
                }
                state.store(ui.ctx());
                ui.ctx().request_repaint();
            } else if ui
                .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft))
                && let Some(entry) = index.and_then(|index| visible.get(index))
            {
                if let Some(id) = entry.collapsing_id {
                    let mut state =
                        egui::collapsing_header::CollapsingState::load_with_default_open(
                            ui.ctx(),
                            id,
                            false,
                        );
                    if state.is_open() {
                        state.set_open(false);
                        state.store(ui.ctx());
                        ui.ctx().request_repaint();
                    } else if let Some(parent) = entry.path.parent()
                        && visible.iter().any(|item| item.path == parent)
                    {
                        self.focused_path = Some(parent.to_path_buf());
                        self.focus_pending = true;
                        ui.ctx().request_repaint();
                    }
                } else if let Some(parent) = entry.path.parent()
                    && visible.iter().any(|item| item.path == parent)
                {
                    self.focused_path = Some(parent.to_path_buf());
                    self.focus_pending = true;
                    ui.ctx().request_repaint();
                }
            }
        }
        actions
    }
}

fn checked_directory(workspace: &Path, path: &Path) -> Result<PathBuf, ProjectError> {
    let check = || -> std::io::Result<PathBuf> {
        let relative = path.strip_prefix(workspace).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Directory is outside the workspace",
            )
        })?;
        // Inspect every component from the anchored workspace root. Checking
        // only the leaf would miss an ancestor replaced with a directory link.
        let mut current = workspace.to_path_buf();
        for component in std::iter::once(None).chain(relative.components().map(Some)) {
            if let Some(component) = component {
                current.push(component);
            }
            let metadata = fs::symlink_metadata(&current)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!(
                        "Directory is no longer traversable without following links: {}",
                        current.display()
                    ),
                ));
            }
        }
        let resolved = path.canonicalize()?;
        if !resolved.starts_with(workspace) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Resolved directory is outside the workspace",
            ));
        }
        Ok(resolved)
    };
    check().map_err(|source| ProjectError {
        path: path.to_path_buf(),
        source,
    })
}

fn read_entries(workspace: &Path, root: &Path) -> Result<Vec<TreeEntry>, ProjectError> {
    let resolved = checked_directory(workspace, root)?;
    let directory = fs::read_dir(&resolved).map_err(|source| ProjectError {
        path: root.to_path_buf(),
        source,
    })?;
    let mut entries = Vec::new();
    for result in directory {
        let entry = match result {
            Ok(entry) => entry,
            Err(error) => {
                entries.push(TreeEntry {
                    path: root.to_path_buf(),
                    name: "Unreadable entry".into(),
                    directory: false,
                    symlink: false,
                    children: None,
                    loading_request: None,
                    error: Some(error.to_string()),
                });
                continue;
            }
        };
        let name = entry.file_name();
        if EXCLUDED.iter().any(|excluded| name == *excluded) {
            continue;
        }
        let (directory, symlink, error) = match entry.file_type() {
            Ok(kind) => (kind.is_dir(), kind.is_symlink(), None),
            Err(error) => (false, false, Some(error.to_string())),
        };
        entries.push(TreeEntry {
            path: entry.path(),
            name: name.to_string_lossy().into_owned(),
            directory,
            symlink,
            children: None,
            loading_request: None,
            error,
        });
    }
    // Keep exact OS names for ordering and identity even when labels are lossy.
    entries.sort_by(|left, right| {
        right
            .directory
            .cmp(&left.directory)
            .then_with(|| left.path.file_name().cmp(&right.path.file_name()))
    });
    Ok(entries)
}

fn read_visible_entries(workspace: &Path, root: &Path) -> Result<Vec<TreeEntry>, ProjectError> {
    let mut entries = read_entries(workspace, root)?;
    if is_java_package_context(root) {
        for entry in &mut entries {
            compact_package(workspace, entry);
        }
    }
    Ok(entries)
}

fn is_java_package_context(path: &Path) -> bool {
    path.ancestors().any(|ancestor| {
        ancestor.file_name().is_some_and(|name| name == "java")
            && ancestor
                .ancestors()
                .skip(1)
                .take(2)
                .any(|parent| parent.file_name().is_some_and(|name| name == "src"))
    })
}

fn compact_package(workspace: &Path, entry: &mut TreeEntry) {
    if !entry.directory || entry.symlink || entry.error.is_some() {
        return;
    }

    let mut segments = vec![entry.name.clone()];
    let mut path = entry.path.clone();
    loop {
        match read_entries(workspace, &path) {
            Ok(mut children)
                if children.len() == 1
                    && children[0].directory
                    && !children[0].symlink
                    && children[0].error.is_none() =>
            {
                let child = children.pop().expect("one package child");
                segments.push(child.name);
                path = child.path;
            }
            Ok(children) => {
                entry.name = segments.join(".");
                entry.path = path;
                entry.children = Some(children);
                return;
            }
            Err(error) => {
                entry.name = segments.join(".");
                entry.path = path;
                entry.error = Some(error.to_string());
                return;
            }
        }
    }
}

fn find_entry_mut<'a>(entries: &'a mut [TreeEntry], path: &Path) -> Option<&'a mut TreeEntry> {
    for entry in entries {
        if entry.path == path {
            return Some(entry);
        }
        if let Some(children) = &mut entry.children {
            if let Some(entry) = find_entry_mut(children, path) {
                return Some(entry);
            }
        }
    }
    None
}

fn find_entry<'a>(entries: &'a [TreeEntry], path: &Path) -> Option<&'a TreeEntry> {
    for entry in entries {
        if entry.path == path {
            return Some(entry);
        }
        if let Some(found) = entry
            .children
            .as_deref()
            .and_then(|children| find_entry(children, path))
        {
            return Some(found);
        }
    }
    None
}

fn merge_entries(previous: Vec<TreeEntry>, mut refreshed: Vec<TreeEntry>) -> Vec<TreeEntry> {
    for entry in &mut refreshed {
        if let Some(old) = previous.iter().find(|old| {
            old.path == entry.path
                && old.directory
                && entry.directory
                && !old.symlink
                && !entry.symlink
        }) {
            entry.children = old.children.clone();
            entry.loading_request = old.loading_request;
            entry.error = old.error.clone();
        }
    }
    refreshed
}

fn collect_expanded_paths(entries: &[TreeEntry], paths: &mut Vec<PathBuf>) {
    for entry in entries {
        if entry.directory && !entry.symlink && entry.children.is_some() {
            paths.push(entry.path.clone());
            if let Some(children) = &entry.children {
                collect_expanded_paths(children, paths);
            }
        }
    }
}

fn context_menu(
    ui: &mut egui::Ui,
    path: &Path,
    directory: bool,
    symlink: bool,
    actions: &mut Vec<TreeAction>,
) {
    if directory
        && !symlink
        && ui
            .button(crate::i18n::ui_text(ui, "New Java Class…"))
            .clicked()
    {
        actions.push(TreeAction::NewJavaClass(path.to_path_buf()));
        ui.close();
    }
    if directory && ui.button(crate::i18n::ui_text(ui, "Refresh")).clicked() {
        actions.push(TreeAction::Refresh(path.to_path_buf()));
        ui.close();
    }
    if ui
        .button(crate::i18n::ui_text(ui, "Reveal in Finder"))
        .clicked()
    {
        actions.push(TreeAction::Reveal(path.to_path_buf()));
        ui.close();
    }
}

fn render_entries(
    ui: &mut egui::Ui,
    workspace: &Path,
    entries: &mut [TreeEntry],
    actions: &mut Vec<TreeAction>,
    focused_path: &mut Option<PathBuf>,
    focus_pending: &mut bool,
    visible: &mut Vec<VisibleProjectEntry>,
    has_focus: &mut bool,
) {
    for entry in entries {
        if entry.directory || entry.error.is_none() {
            visible.push(VisibleProjectEntry {
                path: entry.path.clone(),
                name: entry.name.clone(),
                collapsing_id: entry.directory.then(|| ui.make_persistent_id(&entry.path)),
            });
        }
        if entry.directory {
            let id = ui.make_persistent_id(&entry.path);
            let header = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                id,
                false,
            )
            .show_header(ui, |ui| {
                ui.selectable_label(focused_path.as_ref() == Some(&entry.path), &entry.name)
            });
            let (toggle_response, header_response, _) = header.body(|ui| {
                if entry.children.is_none()
                    && entry.error.is_none()
                    && entry.loading_request.is_none()
                {
                    let request = NEXT_TREE_REQUEST.fetch_add(1, Ordering::Relaxed);
                    entry.loading_request = Some(request);
                    actions.push(TreeAction::LoadChildren {
                        path: entry.path.clone(),
                        request,
                    });
                }
                if let Some(error) = &entry.error {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        crate::i18n::message(ui.ctx(), error),
                    );
                }
                if entry.loading_request.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(crate::i18n::ui_text(ui, "Loading…"));
                    });
                }
                if let Some(children) = &mut entry.children {
                    if children.is_empty() {
                        ui.weak(crate::i18n::ui_text(ui, "Empty directory"));
                    }
                    render_entries(
                        ui,
                        workspace,
                        children,
                        actions,
                        focused_path,
                        focus_pending,
                        visible,
                        has_focus,
                    );
                }
            });
            if toggle_response.clicked() {
                *focused_path = Some(entry.path.clone());
                *focus_pending = true;
            }
            if focused_path.as_ref() == Some(&entry.path) && *focus_pending {
                header_response.inner.request_focus();
                *focus_pending = false;
            }
            if header_response.inner.has_focus() {
                *has_focus = true;
            }
            if header_response.inner.clicked() {
                *focused_path = Some(entry.path.clone());
            }
            header_response.response.context_menu(|ui| {
                context_menu(ui, &entry.path, true, entry.symlink, actions);
            });
        } else if let Some(error) = &entry.error {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("{}: {error}", entry.name),
            );
        } else {
            // Symlink directories are leaves, preventing cycles or an accidental
            // traversal outside the opened workspace. File links still open.
            let label = if entry.symlink {
                format!("{} ↗", entry.name)
            } else {
                entry.name.clone()
            };
            let response = ui
                .selectable_label(focused_path.as_ref() == Some(&entry.path), label)
                .on_hover_text(entry.path.display().to_string());
            if focused_path.as_ref() == Some(&entry.path) && *focus_pending {
                response.request_focus();
                *focus_pending = false;
            }
            if response.has_focus() {
                *has_focus = true;
            }
            if response.clicked() {
                *focused_path = Some(entry.path.clone());
                actions.push(TreeAction::OpenFile(entry.path.clone()));
            }
            response
                .context_menu(|ui| context_menu(ui, &entry.path, false, entry.symlink, actions));
        }
    }
}

struct VisibleProjectEntry {
    path: PathBuf,
    name: String,
    collapsing_id: Option<egui::Id>,
}
