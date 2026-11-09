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
    tree_focused: bool,
    typeahead_query: String,
    typeahead_at: Option<Instant>,
    typeahead_visible_match: bool,
    reveal_path: Option<PathBuf>,
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
    java_kind: Option<JavaKind>,
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
            tree_focused: false,
            typeahead_query: String::new(),
            typeahead_at: None,
            typeahead_visible_match: false,
            reveal_path: None,
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

    pub fn typeahead_query(&self) -> Option<&str> {
        (self.tree_focused && !self.typeahead_query.is_empty()).then_some(&self.typeahead_query)
    }

    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        focus_requested: &mut bool,
        maven_modules: &[super::ProjectModule],
        search_result: Option<(&str, &Path)>,
    ) -> Vec<TreeAction> {
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
            self.tree_focused = false;
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
            maven_modules,
            &mut actions,
            &mut self.focused_path,
            &mut self.focus_pending,
            &mut visible,
            &mut has_focus,
        );
        if let Some((query, path)) = search_result
            && query == self.typeahead_query
            && !self.typeahead_visible_match
            && path.starts_with(&self.root)
        {
            self.reveal_path = Some(path.to_path_buf());
        }
        if let Some(target) = self.reveal_path.clone() {
            if visible.iter().any(|entry| entry.path == target) {
                if self.focused_path.as_ref() != Some(&target) {
                    self.focused_path = Some(target);
                    self.focus_pending = true;
                    ui.ctx().request_repaint();
                }
                self.reveal_path = None;
            } else if let Some(id) = visible.iter().find_map(|entry| {
                let id = entry.collapsing_id?;
                if !target.starts_with(&entry.path) || target == entry.path {
                    return None;
                }
                let state = egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    id,
                    false,
                );
                (!state.is_open()).then_some(id)
            }) {
                let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    id,
                    false,
                );
                state.set_open(true);
                state.store(ui.ctx());
                ui.ctx().request_repaint();
            }
        }
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
            let continuing = self
                .typeahead_at
                .is_some_and(|last| last.elapsed() <= Duration::from_secs(1));
            let backspace = ui.input_mut(|input| {
                !self.typeahead_query.is_empty()
                    && input.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)
            });
            let escape = ui.input_mut(|input| {
                !self.typeahead_query.is_empty()
                    && input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
            });
            let mut search_changed = false;
            if escape {
                self.typeahead_query.clear();
                self.typeahead_at = None;
                self.typeahead_visible_match = false;
                self.reveal_path = None;
            } else {
                if backspace {
                    search_changed = self.typeahead_query.pop().is_some();
                }
                if !typed.is_empty() {
                    if !continuing {
                        self.typeahead_query.clear();
                    }
                    self.typeahead_query.push_str(&typed);
                    search_changed = true;
                }
            }
            if search_changed {
                // shortcut: reuses the 100,000-file Quick Open index; raise its cap for larger workspaces.
                self.typeahead_at = (!self.typeahead_query.is_empty()).then(Instant::now);
                self.reveal_path = None;
                let query = self.typeahead_query.to_lowercase();
                if !query.is_empty() && !visible.is_empty() {
                    let start = if backspace || continuing {
                        index.unwrap_or(0)
                    } else {
                        index.map_or(0, |index| (index + 1) % visible.len())
                    };
                    let next = (0..visible.len())
                        .map(|offset| (start + offset) % visible.len())
                        .find(|index| visible[*index].name.to_lowercase().starts_with(&query));
                    self.typeahead_visible_match = next.is_some();
                    if let Some(next) = next {
                        self.focused_path = Some(visible[next].path.clone());
                        self.focus_pending = true;
                        ui.ctx().request_repaint();
                    }
                } else {
                    self.typeahead_visible_match = false;
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
        self.tree_focused = has_focus;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JavaKind {
    Class,
    Abstract,
    Interface,
    Enum,
}

#[derive(Clone, Copy)]
enum TreeIcon {
    Source,
    Resources,
    TestRoot,
    Package,
    Maven,
    Gradle,
    Java(JavaKind),
}

fn folder_kind(path: &Path) -> Option<TreeIcon> {
    let parent = path.parent()?;
    if path.file_name()? == "test" && parent.file_name()? == "src" {
        return Some(TreeIcon::TestRoot);
    }
    if path.file_name()? != "java" && is_java_package_context(path) {
        return Some(TreeIcon::Package);
    }
    if !["main", "test"]
        .iter()
        .any(|name| parent.file_name().is_some_and(|p| p == *name))
        || parent.parent()?.file_name()? != "src"
    {
        return None;
    }
    match path.file_name()?.to_str()? {
        "java" => Some(TreeIcon::Source),
        "resources" => Some(TreeIcon::Resources),
        _ => None,
    }
}

fn is_test_path(path: &Path) -> bool {
    path.ancestors().any(|p| {
        p.file_name().is_some_and(|n| n == "test")
            && p.parent()
                .and_then(Path::file_name)
                .is_some_and(|n| n == "src")
    })
}

// shortcut: lexical scanning does not resolve Java Unicode escapes; use a Java parser if needed.
fn java_kind(source: &str, filename: &str) -> Option<JavaKind> {
    let name = filename.strip_suffix(".java")?;
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"//") {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if bytes[i..].starts_with(b"/*") {
            i += 2;
            while i + 1 < bytes.len() && &bytes[i..i + 2] != b"*/" {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
        } else if bytes[i..].starts_with(b"\"\"\"") {
            i += 3;
            while i + 2 < bytes.len() && &bytes[i..i + 3] != b"\"\"\"" {
                if bytes[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 3).min(bytes.len());
        } else if bytes[i] == b'"' || bytes[i] == b'\'' {
            let quote = bytes[i];
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i = (i + 2).min(bytes.len());
                } else if bytes[i] == quote {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
        } else if bytes[i].is_ascii_alphanumeric()
            || bytes[i] == b'_'
            || bytes[i] == b'$'
            || bytes[i] >= 128
        {
            let start = i;
            i += 1;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric()
                    || bytes[i] == b'_'
                    || bytes[i] == b'$'
                    || bytes[i] >= 128)
            {
                i += 1;
            }
            if let Some(token) = source.get(start..i) {
                tokens.push(token);
            }
        } else {
            if matches!(bytes[i], b'{' | b'}' | b';' | b'@' | b'(' | b')') {
                tokens.push(&source[i..i + 1]);
            }
            i += 1;
        }
    }
    let mut depth = 0usize;
    let mut abstract_modifier = false;
    let mut parentheses = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match *token {
            "(" => {
                parentheses += 1;
            }
            ")" => {
                parentheses = parentheses.saturating_sub(1);
            }
            _ if parentheses > 0 => {}
            "{" => {
                depth += 1;
                abstract_modifier = false;
            }
            "}" => {
                depth = depth.saturating_sub(1);
                abstract_modifier = false;
            }
            ";" if depth == 0 => abstract_modifier = false,
            "abstract" if depth == 0 => abstract_modifier = true,
            "class" | "interface" | "enum"
                if depth == 0 && tokens.get(index + 1) == Some(&name) =>
            {
                return Some(match *token {
                    "interface" => JavaKind::Interface,
                    "enum" => JavaKind::Enum,
                    _ if abstract_modifier => JavaKind::Abstract,
                    _ => JavaKind::Class,
                });
            }
            _ => {}
        }
    }
    None
}

fn paint_tree_icon(ui: &egui::Ui, row: egui::Rect, icon: TreeIcon, test: bool) {
    let painter = ui.painter();
    let center = egui::pos2(row.left() + 11.0, row.center().y);
    if test {
        painter.rect_filled(
            egui::Rect::from_center_size(center, egui::vec2(18.0, 18.0)),
            4.0,
            egui::Color32::from_rgb(35, 80, 51),
        );
    }
    match icon {
        TreeIcon::Source | TreeIcon::Resources | TreeIcon::TestRoot => {
            let color = if test {
                egui::Color32::from_rgb(111, 199, 116)
            } else if matches!(icon, TreeIcon::Source) {
                egui::Color32::from_rgb(75, 157, 225)
            } else {
                egui::Color32::from_rgb(150, 160, 171)
            };
            let tab =
                egui::Rect::from_min_size(center + egui::vec2(-7.0, -6.0), egui::vec2(7.0, 4.0));
            let body =
                egui::Rect::from_min_size(center + egui::vec2(-7.0, -3.5), egui::vec2(14.0, 9.0));
            painter.rect_filled(tab, 1.0, color);
            painter.rect_filled(body, 1.5, color);
            if matches!(icon, TreeIcon::Resources) {
                for y in [0.0, 2.0, 4.0] {
                    painter.line_segment(
                        [center + egui::vec2(1.0, y), center + egui::vec2(6.0, y)],
                        egui::Stroke::new(1.0, egui::Color32::WHITE),
                    );
                }
            }
        }
        TreeIcon::Package => {
            let color = if test {
                egui::Color32::from_rgb(144, 224, 153)
            } else {
                egui::Color32::from_rgb(207, 166, 92)
            };
            let top =
                egui::Rect::from_min_size(center + egui::vec2(-6.0, -6.0), egui::vec2(12.0, 12.0));
            painter.rect_stroke(
                top,
                2.0,
                egui::Stroke::new(1.5, color),
                egui::StrokeKind::Middle,
            );
            painter.circle_filled(center, 2.0, color);
        }
        TreeIcon::Maven | TreeIcon::Gradle => {
            let (letter, color) = match icon {
                TreeIcon::Maven => ('M', egui::Color32::from_rgb(220, 91, 61)),
                _ => ('G', egui::Color32::from_rgb(76, 145, 89)),
            };
            painter.circle_filled(center, 8.0, color);
            painter.text(
                center,
                egui::Align2::CENTER_CENTER,
                letter,
                egui::FontId::proportional(11.0),
                egui::Color32::WHITE,
            );
        }
        TreeIcon::Java(kind) => {
            let (letter, color) = match kind {
                JavaKind::Class | JavaKind::Abstract => {
                    ("C", egui::Color32::from_rgb(91, 184, 237))
                }
                JavaKind::Interface => ("I", egui::Color32::from_rgb(111, 199, 116)),
                JavaKind::Enum => ("E", egui::Color32::from_rgb(190, 139, 228)),
            };
            let color = if test {
                egui::Color32::from_rgb(144, 224, 153)
            } else {
                color
            };
            if kind == JavaKind::Abstract {
                for n in 0..8 {
                    let a = n as f32 * std::f32::consts::TAU / 8.0;
                    let b = a + 0.48;
                    painter.line_segment(
                        [
                            center + egui::vec2(a.cos(), a.sin()) * 7.0,
                            center + egui::vec2(b.cos(), b.sin()) * 7.0,
                        ],
                        egui::Stroke::new(1.1, color),
                    );
                }
            } else {
                painter.circle_stroke(center, 7.0, egui::Stroke::new(1.1, color));
            }
            painter.text(
                center,
                egui::Align2::CENTER_CENTER,
                letter,
                egui::FontId::proportional(10.0),
                color,
            );
        }
    }
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
                    java_kind: None,
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
        let (directory, symlink, regular_file, error) = match entry.file_type() {
            Ok(kind) => (kind.is_dir(), kind.is_symlink(), kind.is_file(), None),
            Err(error) => (false, false, false, Some(error.to_string())),
        };
        entries.push(TreeEntry {
            path: entry.path(),
            name: name.to_string_lossy().into_owned(),
            directory,
            symlink,
            java_kind: if regular_file
                && !symlink
                && entry.path().extension().is_some_and(|ext| ext == "java")
            {
                fs::read_to_string(entry.path())
                    .ok()
                    .and_then(|source| java_kind(&source, &name.to_string_lossy()))
            } else {
                None
            },
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
    maven_modules: &[super::ProjectModule],
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
            let is_maven_module = maven_modules.iter().any(|module| module.root == entry.path);
            let icon = folder_kind(&entry.path);
            let label = if is_maven_module {
                egui::RichText::new(format!("📦 {}", entry.name)).strong()
            } else {
                egui::RichText::new(if icon.is_some() {
                    format!("     {}", entry.name)
                } else {
                    entry.name.clone()
                })
            };
            let header = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                id,
                false,
            )
            .show_header(ui, |ui| {
                ui.add(
                    egui::Button::selectable(focused_path.as_ref() == Some(&entry.path), label)
                        .wrap_mode(egui::TextWrapMode::Extend),
                )
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
                        maven_modules,
                        actions,
                        focused_path,
                        focus_pending,
                        visible,
                        has_focus,
                    );
                }
            });
            if let Some(icon) = icon.filter(|_| !is_maven_module) {
                paint_tree_icon(
                    ui,
                    header_response.inner.rect,
                    icon,
                    is_test_path(&entry.path),
                );
            }
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
            let has_icon = entry.java_kind.is_some()
                || matches!(entry.name.as_str(), "pom.xml" | "build.gradle");
            let label = if has_icon {
                format!("     {label}")
            } else {
                label
            };
            let response = ui
                .add(
                    egui::Button::selectable(focused_path.as_ref() == Some(&entry.path), label)
                        .wrap_mode(egui::TextWrapMode::Truncate),
                )
                .on_hover_text(entry.path.display().to_string());
            let icon = match entry.name.as_str() {
                "pom.xml" => Some(TreeIcon::Maven),
                "build.gradle" => Some(TreeIcon::Gradle),
                _ => entry.java_kind.map(TreeIcon::Java),
            };
            if let Some(icon) = icon {
                paint_tree_icon(ui, response.rect, icon, is_test_path(&entry.path));
            }
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

#[cfg(test)]
mod icon_tests {
    use super::*;
    #[test]
    fn icons_emit_expected_shapes() {
        let ctx = egui::Context::default();
        for (icon, circles, lines, texts) in [
            (TreeIcon::Source, 0, 0, 0),
            (TreeIcon::Resources, 0, 3, 0),
            (TreeIcon::Java(JavaKind::Class), 1, 0, 1),
            (TreeIcon::Java(JavaKind::Interface), 1, 0, 1),
            (TreeIcon::Java(JavaKind::Enum), 1, 0, 1),
            (TreeIcon::Java(JavaKind::Abstract), 0, 8, 1),
        ] {
            let mut output = ctx.run_ui(egui::RawInput::default(), |root| {
                egui::CentralPanel::default().show(root, |ui| {
                    paint_tree_icon(
                        ui,
                        egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(100.0, 20.0)),
                        icon,
                        false,
                    );
                });
            });
            output.textures_delta.clear();
            assert_eq!(
                output
                    .shapes
                    .iter()
                    .filter(|s| matches!(s.shape, egui::Shape::Circle(_)))
                    .count(),
                circles
            );
            assert_eq!(
                output
                    .shapes
                    .iter()
                    .filter(|s| matches!(s.shape, egui::Shape::LineSegment { .. }))
                    .count(),
                lines
            );
            assert_eq!(
                output
                    .shapes
                    .iter()
                    .filter(|s| matches!(s.shape, egui::Shape::Text(_)))
                    .count(),
                texts
            );
        }
    }

    #[test]
    fn test_icons_have_green_background_and_foreground() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |root| {
            egui::CentralPanel::default().show(root, |ui| {
                paint_tree_icon(
                    ui,
                    egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(100.0, 20.0)),
                    TreeIcon::Java(JavaKind::Enum),
                    true,
                );
            });
        });
        output.textures_delta.clear();
        assert!(output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.fill == egui::Color32::from_rgb(35, 80, 51))));
        assert!(output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Circle(c) if c.stroke.color == egui::Color32::from_rgb(144, 224, 153))));
    }

    #[test]
    fn tree_rows_render_source_and_java_icons() {
        let ctx = egui::Context::default();
        let mut entries = vec![
            TreeEntry {
                path: PathBuf::from("src/main/java"),
                name: "java".into(),
                directory: true,
                symlink: false,
                java_kind: None,
                children: None,
                loading_request: None,
                error: None,
            },
            TreeEntry {
                path: PathBuf::from("src/main/resources"),
                name: "resources".into(),
                directory: true,
                symlink: false,
                java_kind: None,
                children: None,
                loading_request: None,
                error: None,
            },
            TreeEntry {
                path: PathBuf::from("Foo.java"),
                name: "Foo.java".into(),
                directory: false,
                symlink: false,
                java_kind: Some(JavaKind::Class),
                children: None,
                loading_request: None,
                error: None,
            },
            TreeEntry {
                path: PathBuf::from("Bar.java"),
                name: "Bar.java".into(),
                directory: false,
                symlink: false,
                java_kind: Some(JavaKind::Abstract),
                children: None,
                loading_request: None,
                error: None,
            },
            TreeEntry {
                path: PathBuf::from("I.java"),
                name: "I.java".into(),
                directory: false,
                symlink: false,
                java_kind: Some(JavaKind::Interface),
                children: None,
                loading_request: None,
                error: None,
            },
            TreeEntry {
                path: PathBuf::from("E.java"),
                name: "E.java".into(),
                directory: false,
                symlink: false,
                java_kind: Some(JavaKind::Enum),
                children: None,
                loading_request: None,
                error: None,
            },
        ];
        let mut output = ctx.run_ui(egui::RawInput::default(), |root| {
            egui::CentralPanel::default().show(root, |ui| {
                render_entries(
                    ui,
                    Path::new("."),
                    &mut entries,
                    &[],
                    &mut vec![],
                    &mut None,
                    &mut false,
                    &mut vec![],
                    &mut false,
                );
            });
        });
        output.textures_delta.clear();
        assert_eq!(
            output
                .shapes
                .iter()
                .filter(|s| matches!(s.shape, egui::Shape::Circle(_)))
                .count(),
            3
        );
        assert_eq!(
            output
                .shapes
                .iter()
                .filter(|s| matches!(s.shape, egui::Shape::LineSegment { .. }))
                .count(),
            11
        );
    }

    #[test]
    fn directory_loading_classifies_java_files() {
        let root = std::env::temp_dir().join(format!(
            "intelli-gen-icon-test-{}-{}",
            std::process::id(),
            NEXT_TREE_REQUEST.fetch_add(1, Ordering::Relaxed)
        ));
        let dir = root.join("src/main/java");
        fs::create_dir_all(&dir).unwrap();
        for (name, source) in [
            ("A.java", "class A {}"),
            ("B.java", "abstract class B {}"),
            ("C.java", "interface C {}"),
            ("D.java", "enum D {}"),
            ("other.txt", "class other {}"),
        ] {
            fs::write(dir.join(name), source).unwrap();
        }
        let root = fs::canonicalize(root).unwrap();
        let dir = root.join("src/main/java");
        let entries = read_visible_entries(&root, &dir).unwrap();
        for (name, kind) in [
            ("A.java", Some(JavaKind::Class)),
            ("B.java", Some(JavaKind::Abstract)),
            ("C.java", Some(JavaKind::Interface)),
            ("D.java", Some(JavaKind::Enum)),
            ("other.txt", None),
        ] {
            assert_eq!(
                entries
                    .iter()
                    .find(|entry| entry.name == name)
                    .unwrap()
                    .java_kind,
                kind
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recognizes_source_folders_and_java_declarations() {
        assert!(matches!(
            folder_kind(Path::new("module/src/main/java")),
            Some(TreeIcon::Source)
        ));
        assert!(matches!(
            folder_kind(Path::new("module/src/main/resources")),
            Some(TreeIcon::Resources)
        ));
        assert!(folder_kind(Path::new("other/main/java")).is_none());
        assert!(matches!(
            folder_kind(Path::new("module/src/test")),
            Some(TreeIcon::TestRoot)
        ));
        assert!(matches!(
            folder_kind(Path::new("module/src/test/java")),
            Some(TreeIcon::Source)
        ));
        assert!(matches!(
            folder_kind(Path::new("module/src/test/resources")),
            Some(TreeIcon::Resources)
        ));
        assert!(is_test_path(Path::new("module/src/test/java/Foo.java")));
        assert!(!is_test_path(Path::new("module/src/main/java/Foo.java")));
        assert!(!is_test_path(Path::new("module/src/testdata/Foo.java")));
        assert_eq!(
            java_kind("// class Foo {}\n public abstract class Foo {}", "Foo.java"),
            Some(JavaKind::Abstract)
        );
        assert_eq!(
            java_kind("interface Foo {}", "Foo.java"),
            Some(JavaKind::Interface)
        );
        assert_eq!(java_kind("enum Foo {}", "Foo.java"), Some(JavaKind::Enum));
        assert_eq!(
            java_kind(
                "abstract @SuppressWarnings({\"x\", \"y\"}) class Foo {}",
                "Foo.java"
            ),
            Some(JavaKind::Abstract)
        );
        assert_eq!(
            java_kind("class Other { class Foo {} } class Foo {}", "Foo.java"),
            Some(JavaKind::Class)
        );
        assert_eq!(java_kind("class Other { class Foo {} }", "Foo.java"), None);
    }
}
