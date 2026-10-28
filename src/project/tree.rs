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
    root_error: Option<String>,
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
            root_error: None,
            root_validation_request: None,
            next_root_validation: Instant::now() + Duration::from_secs(1),
        })
    }

    pub fn begin_refresh(&mut self, path: &Path) -> Option<u64> {
        let entry = find_entry_mut(&mut self.entries, path)?;
        if !entry.directory || entry.symlink {
            return None;
        }
        let request = NEXT_TREE_REQUEST.fetch_add(1, Ordering::Relaxed);
        entry.loading_request = Some(request);
        entry.error = None;
        Some(request)
    }

    pub fn read_children(workspace: &Path, path: &Path, request: u64) -> Result<ProjectTreeChildren, String> {
        read_visible_entries(workspace, path)
            .map(|entries| ProjectTreeChildren { parent: path.to_path_buf(), request, entries })
            .map_err(|error| error.to_string())
    }

    pub fn validate_root(root: &Path) -> Result<(), String> {
        checked_directory(root, root).map(|_| ()).map_err(|error| error.to_string())
    }

    pub fn apply_root_validation(&mut self, request: u64, result: Result<(), String>) {
        if self.root_validation_request != Some(request) { return; }
        self.root_validation_request = None;
        self.root_error = result.err();
        self.next_root_validation = Instant::now() + Duration::from_secs(1);
    }

    pub fn root_validation_delay(&self) -> Option<Duration> {
        self.root_validation_request.is_none().then(|| {
            self.next_root_validation.saturating_duration_since(Instant::now())
        })
    }

    pub fn apply_children(
        &mut self,
        parent: &Path,
        request: u64,
        result: Result<ProjectTreeChildren, String>,
    ) {
        let Some(entry) = find_entry_mut(&mut self.entries, parent) else { return; };
        if entry.loading_request != Some(request) { return; }
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

    pub fn render(&mut self, ui: &mut egui::Ui) -> Vec<TreeAction> {
        let mut actions = Vec::new();
        if Instant::now() >= self.next_root_validation && self.root_validation_request.is_none() {
            let request = NEXT_TREE_REQUEST.fetch_add(1, Ordering::Relaxed);
            self.root_validation_request = Some(request);
            actions.push(TreeAction::ValidateRoot { request });
        }
        if self.entries.is_empty() {
            ui.weak("Empty directory");
        }
        if let Some(error) = &self.root_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
            return actions;
        }
        render_entries(ui, &self.root, &mut self.entries, &mut actions);
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
        if entry.path == path { return Some(entry); }
        if let Some(children) = &mut entry.children {
            if let Some(entry) = find_entry_mut(children, path) { return Some(entry); }
        }
    }
    None
}

fn context_menu(ui: &mut egui::Ui, path: &Path, directory: bool, symlink: bool, actions: &mut Vec<TreeAction>) {
    if directory && !symlink && ui.button("New Java Class…").clicked() {
        actions.push(TreeAction::NewJavaClass(path.to_path_buf()));
        ui.close();
    }
    if directory && ui.button("Refresh").clicked() {
        actions.push(TreeAction::Refresh(path.to_path_buf()));
        ui.close();
    }
    if ui.button("Reveal in Finder").clicked() {
        actions.push(TreeAction::Reveal(path.to_path_buf()));
        ui.close();
    }
}

fn render_entries(
    ui: &mut egui::Ui,
    workspace: &Path,
    entries: &mut [TreeEntry],
    actions: &mut Vec<TreeAction>,
) {
    for entry in entries {
        if entry.directory {
            let response = egui::CollapsingHeader::new(&entry.name)
                .id_salt(&entry.path)
                .show(ui, |ui| {
                    if entry.children.is_none() && entry.error.is_none() && entry.loading_request.is_none() {
                        let request = NEXT_TREE_REQUEST.fetch_add(1, Ordering::Relaxed);
                        entry.loading_request = Some(request);
                        actions.push(TreeAction::LoadChildren { path: entry.path.clone(), request });
                    }
                    if let Some(error) = &entry.error {
                        ui.colored_label(ui.visuals().error_fg_color, error);
                    }
                    if entry.loading_request.is_some() {
                        ui.horizontal(|ui| { ui.spinner(); ui.label("Loading…"); });
                    }
                    if let Some(children) = &mut entry.children {
                        if children.is_empty() {
                            ui.weak("Empty directory");
                        }
                        render_entries(ui, workspace, children, actions);
                    }
                });
            response.header_response.context_menu(|ui| {
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
                .selectable_label(false, label)
                .on_hover_text(entry.path.display().to_string());
            if response.clicked() {
                actions.push(TreeAction::OpenFile(entry.path.clone()));
            }
            response.context_menu(|ui| context_menu(ui, &entry.path, false, entry.symlink, actions));
        }
    }
}
