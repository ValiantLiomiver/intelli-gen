use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};

use eframe::egui;

struct Entry {
    path: PathBuf,
    display: String,
    folded: String,
    filename: String,
}

pub struct QuickOpen {
    root: Option<PathBuf>,
    indexed_root: Option<PathBuf>,
    pending_root: Option<PathBuf>,
    files: Arc<Vec<Entry>>,
    matches: Vec<(usize, i64)>,
    last_query: String,
    selected: usize,
    loading: bool,
    warning: Option<String>,
    error: Option<String>,
    generation: Arc<AtomicU64>,
    sender: mpsc::Sender<QuickOpenEvent>,
    receiver: mpsc::Receiver<QuickOpenEvent>,
    search_generation: Arc<AtomicU64>,
    workers: Vec<JoinHandle<()>>,
}

enum QuickOpenEvent {
    Indexed {
        generation: u64,
        root: PathBuf,
        result: Result<(Vec<Entry>, Option<String>), String>,
    },
    Matched {
        generation: u64,
        search_generation: u64,
        results: Vec<(usize, i64)>,
    },
}

impl Default for QuickOpen {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            root: None,
            indexed_root: None,
            pending_root: None,
            files: Arc::new(Vec::new()),
            matches: Vec::new(),
            last_query: String::new(),
            selected: 0,
            loading: false,
            warning: None,
            error: None,
            generation: Arc::new(AtomicU64::new(0)),
            search_generation: Arc::new(AtomicU64::new(0)),
            sender,
            receiver,
            workers: Vec::new(),
        }
    }
}

impl QuickOpen {
    pub fn set_project(&mut self, root: Option<PathBuf>) {
        if self.root == root {
            return;
        }
        self.cancel_index();
        self.root = root;
        self.indexed_root = None;
        self.files = Arc::new(Vec::new());
        self.matches.clear();
        self.last_query = "\0".into();
        self.warning = None;
    }

    pub fn ensure_indexed(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        if self.indexed_root.as_ref() != Some(&root) && self.pending_root.as_ref() != Some(&root) {
            self.start_index(root);
        }
    }

    pub fn cancel(&mut self) {
        self.cancel_index();
        self.indexed_root = None;
        self.files = Arc::new(Vec::new());
        self.matches.clear();
        self.last_query = "\0".into();
        self.warning = None;
    }

    fn cancel_index(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.search_generation.fetch_add(1, Ordering::AcqRel);
        self.pending_root = None;
        self.loading = false;
    }

    fn start_index(&mut self, root: PathBuf) {
        self.cancel_index();
        let generation = self.generation.load(Ordering::Acquire);
        let current = Arc::clone(&self.generation);
        let sender = self.sender.clone();
        let worker_root = root.clone();
        self.loading = true;
        self.pending_root = Some(root);
        self.error = None;
        self.warning = None;
        match thread::Builder::new()
            .name("quick-open-index".into())
            .spawn(move || {
                let result = index_files(&worker_root, &current, generation);
                let _ = sender.send(QuickOpenEvent::Indexed {
                    generation,
                    root: worker_root,
                    result,
                });
            }) {
            Ok(worker) => self.workers.push(worker),
            Err(error) => {
                self.loading = false;
                self.pending_root = None;
                self.error = Some(format!("Cannot start file index: {error}"));
            }
        }
    }

    pub fn poll(&mut self) {
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                QuickOpenEvent::Indexed {
                    generation,
                    root,
                    result,
                } if generation == self.generation.load(Ordering::Acquire)
                    && self.pending_root.as_ref() == Some(&root) =>
                {
                    self.loading = false;
                    self.pending_root = None;
                    match result {
                        Ok((entries, warning)) => {
                            self.files = Arc::new(entries);
                            self.indexed_root = Some(root);
                            self.warning = warning;
                            if !self.last_query.is_empty() {
                                self.last_query = "\0".into();
                            }
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                QuickOpenEvent::Matched {
                    generation,
                    search_generation,
                    results,
                } if generation == self.generation.load(Ordering::Acquire)
                    && search_generation == self.search_generation.load(Ordering::Acquire) =>
                {
                    self.matches = results;
                    self.selected = 0;
                }
                _ => {}
            }
        }
        let mut active = Vec::new();
        for worker in self.workers.drain(..) {
            if worker.is_finished() {
                let _ = worker.join();
            } else {
                active.push(worker);
            }
        }
        self.workers = active;
    }

    pub fn has_workers(&self) -> bool {
        !self.workers.is_empty()
    }

    pub fn reset_selection(&mut self) {
        self.selected = 0;
    }

    pub fn render_results(
        &mut self,
        ui: &mut egui::Ui,
        query: &mut String,
        recent_files: &[PathBuf],
    ) -> Option<PathBuf> {
        let Some(root) = self.root.clone() else {
            ui.weak(crate::i18n::ui_text(ui, "Open a project to search files"));
            return None;
        };
        self.ensure_indexed();
        if query.is_empty() {
            if !self.last_query.is_empty() {
                self.last_query.clear();
                self.matches.clear();
                self.selected = 0;
                self.search_generation.fetch_add(1, Ordering::AcqRel);
            }
            let recent: Vec<_> = recent_files
                .iter()
                .filter(|path| path.starts_with(&root))
                .take(20)
                .collect();
            let (up, down, enter) = ui.input(|input| {
                (
                    input.key_pressed(egui::Key::ArrowUp),
                    input.key_pressed(egui::Key::ArrowDown),
                    input.key_pressed(egui::Key::Enter),
                )
            });
            if up && !recent.is_empty() {
                self.selected = self.selected.saturating_sub(1);
            }
            if down && !recent.is_empty() {
                self.selected = (self.selected + 1).min(recent.len() - 1);
            }
            let mut chosen = if enter {
                recent.get(self.selected).map(|path| (*path).clone())
            } else {
                None
            };
            ui.label(crate::i18n::ui_text(ui, "Recent files"));
            if recent.is_empty() {
                ui.weak(crate::i18n::ui_text(ui, "No recent files in this project"));
            }
            egui::ScrollArea::vertical()
                .max_height(310.0)
                .show(ui, |ui| {
                    for (row, path) in recent.iter().enumerate() {
                        let display = path.strip_prefix(&root).unwrap_or(path).display();
                        let response =
                            ui.selectable_label(row == self.selected, display.to_string());
                        if response.clicked() {
                            chosen = Some((*path).clone());
                        }
                        if response.hovered() {
                            self.selected = row;
                        }
                    }
                });
            return chosen;
        }
        if self.last_query != *query {
            self.update_matches(query);
        }
        let mut chosen = None;
        let (up, down, enter) = ui.input(|input| {
            (
                input.key_pressed(egui::Key::ArrowUp),
                input.key_pressed(egui::Key::ArrowDown),
                input.key_pressed(egui::Key::Enter),
            )
        });
        if up && !self.matches.is_empty() {
            self.selected = self.selected.saturating_sub(1);
        }
        if down && !self.matches.is_empty() {
            self.selected = (self.selected + 1).min(self.matches.len() - 1);
        }
        if let Some((index, _)) = self.matches.get(self.selected)
            && enter
        {
            chosen = Some(self.files[*index].path.clone());
        }
        if self.loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(crate::i18n::ui_text(ui, "Indexing project files…"));
            });
        } else if let Some(error) = &self.error {
            ui.colored_label(
                ui.visuals().error_fg_color,
                crate::i18n::message(ui.ctx(), error),
            );
        } else if self.matches.is_empty() {
            ui.weak(crate::i18n::ui_text(ui, "No matching files"));
        }
        if let Some(warning) = &self.warning {
            ui.weak(crate::i18n::message(ui.ctx(), warning));
        }
        egui::ScrollArea::vertical()
            .max_height(310.0)
            .show(ui, |ui| {
                for (row, (index, _)) in self.matches.iter().enumerate() {
                    let selected = row == self.selected;
                    let response = ui.selectable_label(selected, &self.files[*index].display);
                    if response.clicked() {
                        chosen = Some(self.files[*index].path.clone());
                    }
                    if response.hovered() {
                        self.selected = row;
                    }
                }
            });
        chosen
    }

    pub fn update_matches(&mut self, query: &str) {
        if self.last_query == query {
            return;
        }
        self.last_query = query.to_owned();
        self.matches.clear();
        self.selected = 0;
        let query = query.to_lowercase();
        let files = Arc::clone(&self.files);
        let current_generation = Arc::clone(&self.generation);
        let search_generation = Arc::clone(&self.search_generation);
        let generation = current_generation.load(Ordering::Acquire);
        let revision = search_generation.fetch_add(1, Ordering::AcqRel) + 1;
        let sender = self.sender.clone();
        if files.is_empty() {
            self.matches.clear();
            self.selected = 0;
            return;
        }
        match thread::Builder::new()
            .name("quick-open-filter".into())
            .spawn(move || {
                let mut ranked = Vec::new();
                for (index, entry) in files.iter().enumerate() {
                    if index % 512 == 0 && search_generation.load(Ordering::Acquire) != revision {
                        return;
                    }
                    if let Some(score) = fuzzy_score(&entry.folded, &entry.filename, &query) {
                        ranked.push((index, score));
                    }
                }
                ranked.sort_by(|(left_index, left_score), (right_index, right_score)| {
                    right_score
                        .cmp(left_score)
                        .then_with(|| files[*left_index].display.cmp(&files[*right_index].display))
                });
                ranked.truncate(40);
                let _ = sender.send(QuickOpenEvent::Matched {
                    generation,
                    search_generation: revision,
                    results: ranked,
                });
            }) {
            Ok(worker) => self.workers.push(worker),
            Err(error) => self.error = Some(format!("Cannot start file search: {error}")),
        }
    }

    pub fn first_match_path(&self, query: &str) -> Option<&Path> {
        if self.last_query != query {
            return None;
        }
        self.matches
            .first()
            .and_then(|(index, _)| self.files.get(*index))
            .map(|entry| entry.path.as_path())
    }
}

impl Drop for QuickOpen {
    fn drop(&mut self) {
        self.cancel_index();
        for worker in self.workers.drain(..) {
            if worker.is_finished() {
                let _ = worker.join();
            }
        }
    }
}

fn index_files(
    root: &Path,
    generation: &AtomicU64,
    expected: u64,
) -> Result<(Vec<Entry>, Option<String>), String> {
    let metadata = std::fs::metadata(root)
        .map_err(|error| format!("Cannot open project directory {}: {error}", root.display()))?;
    if !metadata.is_dir() {
        return Err(format!(
            "Project path is not a directory: {}",
            root.display()
        ));
    }
    let mut walker = ignore::WalkBuilder::new(root);
    walker
        .follow_links(false)
        .hidden(false)
        .require_git(false)
        .filter_entry(|entry| {
            !matches!(
                entry.file_name().to_str(),
                Some(".git" | "target" | "build" | ".gradle")
            )
        });
    let mut files = Vec::new();
    let mut warning = None;
    for entry in walker.build() {
        if generation.load(Ordering::Acquire) != expected {
            return Ok((Vec::new(), None));
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                if warning.is_none() {
                    warning = Some(format!(
                        "Some project files could not be indexed ({error}); results may be incomplete."
                    ));
                }
                continue;
            }
        };
        if entry.file_type().is_some_and(|kind| kind.is_file()) {
            let path = entry.into_path();
            let display = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            let folded = display.to_lowercase();
            let filename = path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
                .to_lowercase();
            files.push(Entry {
                path,
                display,
                folded,
                filename,
            });
            if files.len() >= 100_000 {
                warning = Some("File index limit reached; results may be incomplete.".into());
                break;
            }
        }
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok((files, warning))
}

pub(super) fn fuzzy_score(path: &str, filename: &str, query: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let mut cursor = 0;
    let mut previous = None;
    let mut score = 0i64;
    for character in query.chars() {
        let offset = path[cursor..].find(character)?;
        let position = cursor + offset;
        score += if previous == Some(position.saturating_sub(1)) {
            12
        } else {
            2
        };
        if position == 0
            || path
                .as_bytes()
                .get(position.saturating_sub(1))
                .is_some_and(|byte| *byte == b'/' || *byte == b'_' || *byte == b'-')
        {
            score += 8;
        }
        cursor = position + character.len_utf8();
        previous = Some(position);
    }
    score -= path.len().saturating_sub(query.len()) as i64 / 8;
    if filename.starts_with(query) {
        score += 40;
    }
    if path == query {
        score += 100;
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;

    fn render_results(
        root: PathBuf,
        query: &mut String,
        recent_files: &[PathBuf],
        events: Vec<egui::Event>,
    ) -> Option<PathBuf> {
        let context = egui::Context::default();
        let mut quick_open = QuickOpen::default();
        quick_open.root = Some(root);
        let mut selected = None;
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                selected = quick_open.render_results(ui, query, recent_files);
            },
        );
        output.textures_delta.clear();
        selected
    }

    fn press_enter() -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]
    }

    #[test]
    fn empty_file_query_opens_the_most_recent_workspace_file() {
        let mut query = String::new();
        let opened = render_results(
            PathBuf::from("/workspace"),
            &mut query,
            &[
                PathBuf::from("/workspace/src/Main.java"),
                PathBuf::from("/workspace/src/Old.java"),
            ],
            press_enter(),
        );

        assert_eq!(opened, Some(PathBuf::from("/workspace/src/Main.java")));
    }

    #[test]
    fn indexing_completion_preserves_recent_file_selection() {
        let root = PathBuf::from("/workspace");
        let recent_files = [
            PathBuf::from("/workspace/src/Main.java"),
            PathBuf::from("/workspace/src/Old.java"),
        ];
        let mut quick_open = QuickOpen::default();
        quick_open.root = Some(root.clone());
        quick_open.pending_root = Some(root.clone());
        quick_open.loading = true;
        let mut query = String::new();
        let context = egui::Context::default();
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                events: vec![egui::Event::Key {
                    key: egui::Key::ArrowDown,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ui| {
                quick_open.render_results(ui, &mut query, &recent_files);
            },
        );
        output.textures_delta.clear();
        let generation = quick_open.generation.load(Ordering::Acquire);
        quick_open
            .sender
            .send(QuickOpenEvent::Indexed {
                generation,
                root: root.clone(),
                result: Ok((Vec::new(), None)),
            })
            .unwrap();
        quick_open.poll();
        let mut selected = None;
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                events: press_enter(),
                ..Default::default()
            },
            |ui| selected = quick_open.render_results(ui, &mut query, &recent_files),
        );
        output.textures_delta.clear();

        assert_eq!(selected, Some(recent_files[1].clone()));
    }

    #[test]
    fn empty_file_query_ignores_recents_outside_workspace() {
        let mut query = String::new();
        let opened = render_results(
            PathBuf::from("/workspace"),
            &mut query,
            &[PathBuf::from("/other/Project.java")],
            press_enter(),
        );

        assert_eq!(opened, None);
    }

    #[test]
    fn typed_file_query_selects_from_index_not_recent_files() {
        let root = PathBuf::from("/workspace");
        let indexed_file = PathBuf::from("/workspace/src/Main.java");
        let mut quick_open = QuickOpen::default();
        quick_open.root = Some(root.clone());
        quick_open.indexed_root = Some(root.clone());
        quick_open.files = Arc::new(vec![
            Entry {
                path: PathBuf::from("/workspace/src/Other.java"),
                display: "src/Other.java".into(),
                folded: "src/other.java".into(),
                filename: "other.java".into(),
            },
            Entry {
                path: indexed_file.clone(),
                display: "src/Main.java".into(),
                folded: "src/main.java".into(),
                filename: "main.java".into(),
            },
        ]);
        let mut query = "Main".to_owned();
        quick_open.update_matches(&query);
        for _ in 0..100 {
            quick_open.poll();
            if !quick_open.matches.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(quick_open.matches.len(), 1);

        let context = egui::Context::default();
        let mut opened = None;
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                events: press_enter(),
                ..Default::default()
            },
            |ui| opened = quick_open.render_results(ui, &mut query, &[indexed_file.clone()]),
        );
        output.textures_delta.clear();

        assert_eq!(opened, Some(indexed_file));
    }

    #[test]
    fn ignores_file_index_completion_from_previous_project() {
        let old_root = PathBuf::from("/tmp/old-search-project");
        let new_root = PathBuf::from("/tmp/new-search-project");
        let mut quick_open = QuickOpen::default();
        quick_open.root = Some(old_root.clone());
        quick_open.pending_root = Some(old_root.clone());
        quick_open.loading = true;
        let old_generation = quick_open.generation.load(Ordering::Acquire);

        quick_open.set_project(Some(new_root.clone()));
        assert!(
            quick_open
                .sender
                .send(QuickOpenEvent::Indexed {
                    generation: old_generation,
                    root: old_root,
                    result: Ok((Vec::new(), None)),
                })
                .is_ok()
        );
        quick_open.poll();

        assert_eq!(quick_open.root.as_ref(), Some(&new_root));
        assert!(quick_open.indexed_root.is_none());
        assert!(quick_open.files.is_empty());
    }
}
