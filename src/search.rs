//! Literal project search. Disk traversal and bounded reads run on a cancellable worker.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
    mpsc,
};

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

#[derive(Default)]
struct SearchResults {
    matches: Vec<SearchMatch>,
    skipped: usize,
    truncated: bool,
}

pub struct ProjectSearch {
    commands: mpsc::Sender<(u64, PathBuf, SearchQuery)>,
    completions: mpsc::Receiver<(u64, Result<SearchResults, String>)>,
    generation: Arc<AtomicU64>,
    pub query: SearchQuery,
    pub globs_text: String,
    pub matches: Vec<SearchMatch>,
    pub running: bool,
    pub message: String,
    pub focus_requested: bool,
}

impl ProjectSearch {
    pub fn new(context: egui::Context) -> Self {
        let (commands, requests) = mpsc::channel::<(u64, PathBuf, SearchQuery)>();
        let (results, completions) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let current = generation.clone();
        std::thread::spawn(move || {
            while let Ok((id, root, query)) = requests.recv() {
                if current.load(Ordering::Relaxed) != id {
                    continue;
                }
                let result = search(&root, &query, || current.load(Ordering::Relaxed) != id);
                if current.load(Ordering::Relaxed) == id {
                    if results.send((id, result)).is_err() {
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
        }
    }

    pub fn start(&mut self, root: PathBuf, query: SearchQuery) {
        self.cancel();
        self.matches.clear();
        if query.text.is_empty() {
            self.message = "Enter text to search".into();
            return;
        }
        let id = self.generation.load(Ordering::Relaxed);
        self.running = self.commands.send((id, root, query)).is_ok();
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

    pub fn reset(&mut self) {
        self.cancel();
        self.matches.clear();
        self.message.clear();
    }

    pub fn poll(&mut self) {
        for (id, result) in self.completions.try_iter() {
            if id != self.generation.load(Ordering::Relaxed) {
                continue;
            }
            self.running = false;
            match result {
                Ok(results) => {
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
        .filter_entry(|entry| entry.file_name() != ".git");
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

pub fn render(ui: &mut egui::Ui, state: &mut crate::app::AppState) {
    use crate::app::events::{AppEvent, SearchAction};
    let mut action = None;
    ui.horizontal(|ui| {
        let response = ui.add(
            egui::TextEdit::singleline(&mut state.search.query.text).hint_text("Find in project"),
        );
        if state.search.focus_requested {
            response.request_focus();
            state.search.focus_requested = false;
        }
        let submitted =
            response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        ui.checkbox(&mut state.search.query.case_sensitive, "Case sensitive");
        if ui
            .add_enabled(state.project.is_some(), egui::Button::new("Search"))
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
            action = Some(AppEvent::Search(SearchAction::Run(query)));
        }
        if state.search.running && ui.button("Cancel").clicked() {
            action = Some(AppEvent::Search(SearchAction::Cancel));
        }
    });
    ui.add(
        egui::TextEdit::singleline(&mut state.search.globs_text)
            .hint_text("Optional file globs, comma separated: **/*.java, pom.xml"),
    );
    ui.label(if state.project.is_none() {
        "Open a project to search"
    } else {
        &state.search.message
    });
    egui::ScrollArea::both()
        .id_salt("search_results")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let root = state.project.as_ref().map(|project| project.root.as_path());
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
        });
    if let Some(action) = action {
        state.enqueue(action);
    }
}
