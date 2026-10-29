//! Literal project search. Disk traversal and bounded reads run on a cancellable worker.
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
    mpsc,
};

use crate::project::tree::EXCLUDED;
use eframe::egui;
use globset::{Glob, GlobSetBuilder};

#[derive(Clone, Debug, Default)]
pub struct SearchQuery {
    pub text: String,
    pub case_sensitive: bool,
    pub file_globs: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SearchMatch {
    pub path: PathBuf,
    pub line: usize,
    pub preview: String,
}

#[derive(Clone, Debug)]
pub struct OpenDocumentSnapshot {
    pub path: PathBuf,
    pub revision: u64,
    pub text: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ReplacePreviewMatch {
    pub line: usize,
    pub count: usize,
    pub preview: String,
}

#[derive(Clone, Debug)]
pub struct ReplacePreviewFile {
    pub path: PathBuf,
    pub source_text: String,
    pub open_revision: Option<u64>,
    pub recheck_disk: bool,
    pub matches: Vec<ReplacePreviewMatch>,
    pub match_count: usize,
    pub selected: bool,
}

#[derive(Default)]
struct SearchResults {
    matches: Vec<SearchMatch>,
    skipped: usize,
    truncated: bool,
}

#[derive(Default)]
struct ReplaceSearchResults {
    files: Vec<ReplacePreviewFile>,
    skipped: usize,
    truncated_preview: bool,
}

enum SearchResponse {
    Search(SearchResults),
    Replace(ReplaceSearchResults),
}

enum SearchMode {
    Search,
    Replace {
        replacement: String,
        open_documents: Vec<OpenDocumentSnapshot>,
    },
}

struct SearchRequest {
    id: u64,
    root: PathBuf,
    query: SearchQuery,
    mode: SearchMode,
}

pub struct ProjectSearch {
    commands: mpsc::Sender<SearchRequest>,
    completions: mpsc::Receiver<(u64, Result<SearchResponse, String>)>,
    generation: Arc<AtomicU64>,
    pub query: SearchQuery,
    pub globs_text: String,
    pub matches: Vec<SearchMatch>,
    pub running: bool,
    pub message: String,
    pub focus_requested: bool,
    pub replace_mode: bool,
    pub replacement_text: String,
    pub replace_files: Vec<ReplacePreviewFile>,
    pub replace_preview_active: bool,
    pub preview_query: Option<SearchQuery>,
    pub preview_replacement: Option<String>,
}

impl ProjectSearch {
    pub fn new(context: egui::Context) -> Self {
        let (commands, requests) = mpsc::channel::<SearchRequest>();
        let (results, completions) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let current = generation.clone();
        std::thread::spawn(move || {
            while let Ok(request) = requests.recv() {
                if current.load(Ordering::Relaxed) != request.id {
                    continue;
                }
                let result = match request.mode {
                    SearchMode::Search => search(&request.root, &request.query, || {
                        current.load(Ordering::Relaxed) != request.id
                    })
                    .map(SearchResponse::Search),
                    SearchMode::Replace { open_documents, .. } => {
                        preview_replacements(&request.root, &request.query, open_documents, || {
                            current.load(Ordering::Relaxed) != request.id
                        })
                        .map(SearchResponse::Replace)
                    }
                };
                if current.load(Ordering::Relaxed) == request.id {
                    if results.send((request.id, result)).is_err() {
                        break;
                    }
                    context.request_repaint();
                }
            }
        });
        Self {
            commands,
            completions,
            generation,
            query: SearchQuery::default(),
            globs_text: String::new(),
            matches: Vec::new(),
            running: false,
            message: String::new(),
            focus_requested: false,
            replace_mode: false,
            replacement_text: String::new(),
            replace_files: Vec::new(),
            replace_preview_active: false,
            preview_query: None,
            preview_replacement: None,
        }
    }

    pub fn start(&mut self, root: PathBuf, query: SearchQuery) {
        self.start_request(root, query, SearchMode::Search);
    }

    pub fn start_replace_preview(
        &mut self,
        root: PathBuf,
        query: SearchQuery,
        replacement: String,
        open_documents: Vec<OpenDocumentSnapshot>,
    ) {
        self.start_request(
            root,
            query,
            SearchMode::Replace {
                replacement,
                open_documents,
            },
        );
    }

    fn start_request(&mut self, root: PathBuf, query: SearchQuery, mode: SearchMode) {
        let preview_request = match &mode {
            SearchMode::Replace { replacement, .. } => Some(replacement.clone()),
            SearchMode::Search => None,
        };
        self.cancel();
        self.matches.clear();
        self.replace_files.clear();
        self.replace_preview_active = preview_request.is_some();
        self.preview_query = preview_request.as_ref().map(|_| query.clone());
        self.preview_replacement = preview_request;
        if query.text.is_empty() {
            self.message = "Enter text to search".into();
            return;
        }
        let id = self.generation.load(Ordering::Relaxed);
        self.running = self
            .commands
            .send(SearchRequest {
                id,
                root,
                query,
                mode,
            })
            .is_ok();
        self.message = if self.running {
            "Searching…"
        } else {
            "Search worker stopped"
        }
        .into();
    }

    pub fn cancel(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        if self.running {
            self.message = "Search cancelled".into();
        }
        self.running = false;
    }

    pub fn clear_replace_preview(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.running = false;
        self.replace_files.clear();
        self.replace_preview_active = false;
        self.preview_query = None;
        self.preview_replacement = None;
    }

    pub fn take_selected_replace_files(&mut self) -> Vec<ReplacePreviewFile> {
        std::mem::take(&mut self.replace_files)
            .into_iter()
            .filter(|file| file.selected)
            .collect()
    }

    pub fn reset(&mut self) {
        self.cancel();
        self.matches.clear();
        self.replace_files.clear();
        self.replace_preview_active = false;
        self.preview_query = None;
        self.preview_replacement = None;
        self.message.clear();
    }

    pub fn poll(&mut self) {
        for (id, result) in self.completions.try_iter() {
            if id != self.generation.load(Ordering::Relaxed) {
                continue;
            }
            self.running = false;
            match result {
                Ok(SearchResponse::Search(results)) => {
                    self.replace_preview_active = false;
                    self.message = format!(
                        "{} matching lines; {} unreadable, binary, non-UTF-8 or oversized files skipped{}",
                        results.matches.len(),
                        results.skipped,
                        if results.truncated {
                            "; result limit reached (5,000)"
                        } else {
                            ""
                        }
                    );
                    self.matches = results.matches;
                }
                Ok(SearchResponse::Replace(results)) => {
                    let files = results.files.len();
                    let matches: usize = results.files.iter().map(|file| file.match_count).sum();
                    self.message = format!(
                        "{matches} replacements in {files} files; {} unreadable, binary, non-UTF-8 or oversized files skipped",
                        results.skipped
                    );
                    if results.truncated_preview {
                        self.message.push_str("; preview lines limited to 5,000");
                    }
                    self.replace_files = results.files;
                    self.replace_preview_active = true;
                }
                Err(error) => self.message = error,
            }
        }
    }
}

impl Drop for ProjectSearch {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn search(
    root: &Path,
    query: &SearchQuery,
    cancelled: impl Fn() -> bool,
) -> Result<SearchResults, String> {
    let mut builder = GlobSetBuilder::new();
    for glob in &query.file_globs {
        builder.add(Glob::new(glob).map_err(|error| format!("Invalid file glob: {error}"))?);
    }
    let globs = builder.build().map_err(|error| error.to_string())?;
    let needle = if query.case_sensitive {
        query.text.clone()
    } else {
        query.text.to_lowercase()
    };
    let mut results = SearchResults::default();
    let mut walker = ignore::WalkBuilder::new(root);
    walker
        .hidden(false)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            !EXCLUDED
                .iter()
                .any(|excluded| entry.file_name() == *excluded)
        });
    const MAX_FILE: u64 = 16 * 1024 * 1024;
    for entry in walker.build() {
        if cancelled() {
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                results.skipped += 1;
                continue;
            }
        };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let relative = entry.path().strip_prefix(root).unwrap_or(entry.path());
        if !query.file_globs.is_empty() && !globs.is_match(relative) {
            continue;
        }
        let mut bytes = Vec::new();
        let read = std::fs::File::open(entry.path())
            .and_then(|file| file.take(MAX_FILE + 1).read_to_end(&mut bytes));
        if read.is_err() || bytes.len() as u64 > MAX_FILE || bytes.contains(&0) {
            results.skipped += 1;
            continue;
        }
        let text = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            Err(_) => {
                results.skipped += 1;
                continue;
            }
        };
        for (index, line) in text.lines().enumerate() {
            if cancelled() {
                return Ok(results);
            }
            let matched = if query.case_sensitive {
                line.contains(&needle)
            } else {
                line.to_lowercase().contains(&needle)
            };
            if matched {
                results.matches.push(SearchMatch {
                    path: entry.path().to_owned(),
                    line: index + 1,
                    preview: line.chars().take(300).collect(),
                });
                if results.matches.len() == 5_000 {
                    results.truncated = true;
                    return Ok(results);
                }
            }
        }
    }
    Ok(results)
}

fn preview_replacements(
    root: &Path,
    query: &SearchQuery,
    open_documents: Vec<OpenDocumentSnapshot>,
    cancelled: impl Fn() -> bool,
) -> Result<ReplaceSearchResults, String> {
    use crate::editor::find_replace::replace_all_text;

    if query.text.is_empty() {
        return Err("Enter text to replace".into());
    }
    let root = root
        .canonicalize()
        .map_err(|error| format!("Cannot open project {}: {error}", root.display()))?;
    let mut builder = GlobSetBuilder::new();
    for glob in &query.file_globs {
        builder.add(Glob::new(glob).map_err(|error| format!("Invalid file glob: {error}"))?);
    }
    let globs = builder.build().map_err(|error| error.to_string())?;
    let mut buffers: HashMap<_, _> = open_documents
        .into_iter()
        .filter(|document| document.path.starts_with(&root))
        .map(|document| (document.path.clone(), document))
        .collect();
    let mut results = ReplaceSearchResults::default();
    let mut total_bytes = 0usize;
    let mut preview_lines = 0usize;
    let mut walker = ignore::WalkBuilder::new(&root);
    walker
        .hidden(false)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            !EXCLUDED
                .iter()
                .any(|excluded| entry.file_name() == *excluded)
        });
    const MAX_FILE: u64 = 16 * 1024 * 1024;
    for entry in walker.build() {
        if cancelled() {
            return Ok(results);
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                results.skipped += 1;
                continue;
            }
        };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = match entry.path().canonicalize() {
            Ok(path) if path.starts_with(&root) => path,
            _ => {
                results.skipped += 1;
                continue;
            }
        };
        let relative = path.strip_prefix(&root).unwrap_or(&path);
        if !query.file_globs.is_empty() && !globs.is_match(relative) {
            continue;
        }
        let open_document = buffers.remove(&path);
        let open_revision = open_document.as_ref().map(|document| document.revision);
        let recheck_disk = open_document
            .as_ref()
            .is_none_or(|document| document.text.is_none());
        let text = if let Some(text) = open_document.and_then(|document| document.text) {
            text
        } else {
            let mut bytes = Vec::new();
            let read = std::fs::File::open(&path)
                .and_then(|file| file.take(MAX_FILE + 1).read_to_end(&mut bytes));
            if read.is_err() || bytes.len() as u64 > MAX_FILE || bytes.contains(&0) {
                results.skipped += 1;
                continue;
            }
            match String::from_utf8(bytes) {
                Ok(text) => text,
                Err(_) => {
                    results.skipped += 1;
                    continue;
                }
            }
        };
        if text.len() > MAX_FILE as usize || text.as_bytes().contains(&0) {
            results.skipped += 1;
            continue;
        }
        let (_, match_count, _) = replace_all_text(&text, &query.text, "", query.case_sensitive);
        if match_count == 0 {
            continue;
        }
        if results.files.len() >= 200 {
            return Err("Replace preview exceeds the 200-file limit; narrow the file globs".into());
        }
        total_bytes = total_bytes
            .checked_add(text.len())
            .ok_or("Replace preview size overflow")?;
        if total_bytes > crate::editor::MAX_WORKSPACE_EDIT_BYTES {
            return Err(format!(
                "Replace preview exceeds the {} MiB workspace edit limit; narrow the file globs",
                crate::editor::MAX_WORKSPACE_EDIT_BYTES / (1024 * 1024)
            ));
        }
        let mut matches = Vec::new();
        for (line_index, line) in text.lines().enumerate() {
            let (_, count, _) = replace_all_text(line, &query.text, "", query.case_sensitive);
            if count > 0 {
                if preview_lines < 5_000 {
                    matches.push(ReplacePreviewMatch {
                        line: line_index + 1,
                        count,
                        preview: line.chars().take(300).collect(),
                    });
                    preview_lines += 1;
                } else {
                    results.truncated_preview = true;
                }
            }
        }
        results.files.push(ReplacePreviewFile {
            path,
            source_text: text,
            open_revision,
            recheck_disk,
            matches,
            match_count,
            selected: true,
        });
    }
    Ok(results)
}

pub fn render(ui: &mut egui::Ui, state: &mut crate::app::AppState) {
    use crate::app::events::{AppEvent, SearchAction};
    let mut action = None;
    let mut invalidate_preview = false;
    ui.horizontal(|ui| {
        let response = ui.add(
            egui::TextEdit::singleline(&mut state.search.query.text)
                .hint_text(crate::i18n::ui_text(ui, "Find in project")),
        );
        invalidate_preview |= response.changed();
        if state.search.focus_requested {
            response.request_focus();
            state.search.focus_requested = false;
        }
        let submitted =
            response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        invalidate_preview |= ui
            .checkbox(&mut state.search.query.case_sensitive, "Case sensitive")
            .changed();
        let replace_changed = ui
            .checkbox(&mut state.search.replace_mode, "Replace in project")
            .changed();
        invalidate_preview |= replace_changed;
        if state.search.replace_mode {
            let response = ui.add(
                egui::TextEdit::singleline(&mut state.search.replacement_text)
                    .hint_text(crate::i18n::ui_text(ui, "Replace with")),
            );
            invalidate_preview |= response.changed();
        }
        let button = if state.search.replace_mode {
            "Preview replacements"
        } else {
            "Search"
        };
        if ui
            .add_enabled(
                state.project.is_some() && !state.search.running,
                egui::Button::new(button),
            )
            .clicked()
            || submitted
        {
            let mut query = state.search.query.clone();
            query.file_globs = state
                .search
                .globs_text
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect();
            action = Some(AppEvent::Search(if state.search.replace_mode {
                SearchAction::PreviewReplace(query, state.search.replacement_text.clone())
            } else {
                SearchAction::Run(query)
            }));
        }
        if state.search.running && ui.button(crate::i18n::ui_text(ui, "Cancel")).clicked() {
            action = Some(AppEvent::Search(SearchAction::Cancel));
        }
    });
    invalidate_preview |= ui
        .add(
            egui::TextEdit::singleline(&mut state.search.globs_text).hint_text(
                crate::i18n::ui_text(
                    ui,
                    "Optional file globs, comma separated: **/*.java, pom.xml",
                ),
            ),
        )
        .changed();
    if invalidate_preview && state.search.replace_preview_active {
        state.search.clear_replace_preview();
        state.search.message = "Search or replacement changed; preview again".into();
    }
    let status = if state.project.is_none() {
        crate::i18n::ui_text(ui, "Open a project to search")
    } else {
        crate::i18n::message(ui.ctx(), &state.search.message)
    };
    ui.label(status);
    if state.search.replace_preview_active {
        ui.horizontal(|ui| {
            if ui.button(crate::i18n::ui_text(ui, "Select all")).clicked() {
                for file in &mut state.search.replace_files {
                    file.selected = true;
                }
            }
            if ui.button(crate::i18n::ui_text(ui, "Select none")).clicked() {
                for file in &mut state.search.replace_files {
                    file.selected = false;
                }
            }
            let selected = state
                .search
                .replace_files
                .iter()
                .filter(|file| file.selected)
                .count();
            let pending = !state.pending_workspace_replacements.is_empty();
            if ui
                .add_enabled(
                    selected > 0 && !pending,
                    egui::Button::new(format!("Apply to {selected} files")),
                )
                .clicked()
            {
                action = Some(AppEvent::Search(SearchAction::ApplyReplace));
            }
        });
    }
    egui::ScrollArea::both()
        .id_salt("search_results")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let root = state.project.as_ref().map(|project| project.root.as_path());
            if state.search.replace_preview_active {
                for file in &mut state.search.replace_files {
                    let path = root
                        .and_then(|root| file.path.strip_prefix(root).ok())
                        .unwrap_or(&file.path);
                    ui.push_id(file.path.clone(), |ui| {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut file.selected, "");
                            egui::CollapsingHeader::new(format!(
                                "{} — {} replacements",
                                path.display(),
                                file.match_count
                            ))
                            .show(ui, |ui| {
                                for found in &file.matches {
                                    ui.small(format!(
                                        "{}: {}  ×{}",
                                        found.line, found.preview, found.count
                                    ));
                                }
                            });
                        });
                    });
                }
            } else if !state.search.replace_mode {
                for found in &state.search.matches {
                    let path = root
                        .and_then(|root| found.path.strip_prefix(root).ok())
                        .unwrap_or(&found.path);
                    if ui
                        .selectable_label(
                            false,
                            format!("{}:{}  {}", path.display(), found.line, found.preview),
                        )
                        .clicked()
                    {
                        action = Some(AppEvent::OpenFileAtLine(found.path.clone(), found.line));
                    }
                }
            }
        });
    if let Some(action) = action {
        state.enqueue(action);
    }
}
