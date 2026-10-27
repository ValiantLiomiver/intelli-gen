use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::JoinHandle;

#[derive(Clone, Copy, Debug)]
pub enum FindAction {
    Open { replace: bool },
    Next,
    Previous,
    Replace,
    ReplaceAll,
    Close,
}

pub struct FindReplace {
    pub visible: bool,
    pub replace_visible: bool,
    pub query: String,
    pub replacement: String,
    pub case_sensitive: bool,
    pub focus_query: bool,
    pub status: String,
    last_match: Option<(PathBufKey, usize, usize)>,
    generation: u64,
    pending: bool,
    sender: mpsc::Sender<FindCompletion>,
    receiver: mpsc::Receiver<FindCompletion>,
    workers: Vec<JoinHandle<()>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PathBufKey(std::path::PathBuf);

struct FindCompletion {
    generation: u64,
    path: PathBuf,
    revision: u64,
    result: FindResult,
}

enum FindResult {
    Search(Option<(usize, usize, bool)>, bool),
    ReplaceOne(Option<(String, usize, usize)>),
    ReplaceAll(String, usize, Option<usize>),
}

impl Default for FindReplace {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            visible: false,
            replace_visible: false,
            query: String::new(),
            replacement: String::new(),
            case_sensitive: false,
            focus_query: false,
            status: String::new(),
            last_match: None,
            generation: 0,
            pending: false,
            sender,
            receiver,
            workers: Vec::new(),
        }
    }
}

impl FindReplace {
    pub fn render(&mut self, ctx: &egui::Context) -> Vec<FindAction> {
        let mut actions = Vec::new();
        if !self.visible {
            return actions;
        }
        let mut open = self.visible;
        egui::Window::new(if self.replace_visible {
            "Find and Replace"
        } else {
            "Find in File"
        })
        .collapsible(false)
        .resizable(false)
        .fixed_size([500.0, if self.replace_visible { 170.0 } else { 125.0 }])
        .anchor(egui::Align2::RIGHT_TOP, [-18.0, 48.0])
        .open(&mut open)
        .show(ctx, |ui| {
            let query = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("Find")
                    .desired_width(f32::INFINITY),
            );
            if self.focus_query {
                query.request_focus();
                self.focus_query = false;
            }
            if query.changed() {
                self.invalidate();
                self.status.clear();
            }
            if self.replace_visible {
                let replacement = ui.add(
                    egui::TextEdit::singleline(&mut self.replacement)
                        .hint_text("Replace with")
                        .desired_width(f32::INFINITY),
                );
                if replacement.changed() {
                    self.invalidate_pending();
                }
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.pending, egui::Button::new("Previous"))
                    .clicked()
                {
                    actions.push(FindAction::Previous);
                }
                if ui
                    .add_enabled(!self.pending, egui::Button::new("Next"))
                    .clicked()
                {
                    actions.push(FindAction::Next);
                }
                if self.replace_visible {
                    if ui
                        .add_enabled(!self.pending, egui::Button::new("Replace"))
                        .clicked()
                    {
                        actions.push(FindAction::Replace);
                    }
                    if ui
                        .add_enabled(!self.pending, egui::Button::new("Replace All"))
                        .clicked()
                    {
                        actions.push(FindAction::ReplaceAll);
                    }
                }
                if ui
                    .checkbox(&mut self.case_sensitive, "Case sensitive")
                    .changed()
                {
                    self.invalidate();
                }
                if ui.button("×").on_hover_text("Close").clicked() {
                    actions.push(FindAction::Close);
                }
            });
            if !self.status.is_empty() {
                ui.weak(&self.status);
            }
            let (enter, shift_enter, escape) = ui.input(|input| {
                (
                    input.key_pressed(egui::Key::Enter),
                    input.key_pressed(egui::Key::Enter) && input.modifiers.shift,
                    input.key_pressed(egui::Key::Escape),
                )
            });
            if enter {
                if !self.pending {
                    actions.push(if shift_enter {
                        FindAction::Previous
                    } else {
                        FindAction::Next
                    });
                }
            }
            if escape {
                actions.push(FindAction::Close);
            }
        });
        if !open {
            actions.push(FindAction::Close);
        }
        actions
    }

    pub fn apply(&mut self, action: FindAction, tab: Option<&mut crate::editor::EditorTab>) {
        match action {
            FindAction::Open { replace } => {
                self.invalidate_pending();
                self.visible = true;
                self.replace_visible = replace;
                self.focus_query = true;
                self.status.clear();
            }
            FindAction::Close => {
                self.invalidate_pending();
                self.visible = false;
            }
            FindAction::Next | FindAction::Previous => {
                let Some(tab) = tab else {
                    self.status = "Open a file to search".into();
                    return;
                };
                if self.query.is_empty() {
                    self.status = "Enter text to find".into();
                    return;
                }
                if self.pending {
                    return;
                }
                let previous = matches!(action, FindAction::Previous);
                self.start_work(tab, action, previous);
            }
            FindAction::Replace => {
                let Some(tab) = tab else {
                    self.status = "Open a file to replace text".into();
                    return;
                };
                if self.query.is_empty() {
                    self.status = "Enter text to find".into();
                    return;
                }
                if self.pending {
                    return;
                }
                self.start_work(tab, action, false);
            }
            FindAction::ReplaceAll => {
                let Some(tab) = tab else {
                    self.status = "Open a file to replace text".into();
                    return;
                };
                if self.query.is_empty() {
                    self.status = "Enter text to find".into();
                    return;
                }
                if self.pending {
                    return;
                }
                self.start_work(tab, action, false);
            }
        }
    }

    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = false;
        self.last_match = None;
    }

    fn invalidate_pending(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = false;
    }

    fn start_work(&mut self, tab: &crate::editor::EditorTab, action: FindAction, previous: bool) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let path = tab.path.clone();
        let revision = tab.document.revision;
        let text = tab.document.text.clone();
        let query = self.query.clone();
        let replacement = self.replacement.clone();
        let case_sensitive = self.case_sensitive;
        let cursor = tab.document.character_index();
        let last_match = self
            .last_match
            .as_ref()
            .filter(|(last_path, _, _)| last_path.0 == path)
            .map(|(_, start, end)| (*start, *end));
        let sender = self.sender.clone();
        self.pending = true;
        self.status = match action {
            FindAction::Replace => "Replacing…".into(),
            FindAction::ReplaceAll => "Replacing all matches…".into(),
            _ => "Searching…".into(),
        };
        match std::thread::Builder::new()
            .name("editor-find".into())
            .spawn(move || {
                let result = match action {
                    FindAction::Next | FindAction::Previous => FindResult::Search(
                        find_range(&text, &query, case_sensitive, cursor, previous),
                        previous,
                    ),
                    FindAction::Replace => {
                        let current = last_match.filter(|(start, end)| {
                            char_range_matches(&text, *start, *end, &query, case_sensitive)
                        });
                        let found = current.or_else(|| {
                            find_range(&text, &query, case_sensitive, cursor, false)
                                .map(|(start, end, _)| (start, end))
                        });
                        FindResult::ReplaceOne(found.map(|(start, end)| {
                            let mut output = text;
                            replace_char_range(&mut output, start, end, &replacement);
                            (output, start, start + replacement.chars().count())
                        }))
                    }
                    FindAction::ReplaceAll => {
                        let (output, count, first) =
                            replace_all_text(&text, &query, &replacement, case_sensitive);
                        FindResult::ReplaceAll(output, count, first)
                    }
                    FindAction::Open { .. } | FindAction::Close => return,
                };
                let _ = sender.send(FindCompletion {
                    generation,
                    path,
                    revision,
                    result,
                });
            }) {
            Ok(worker) => self.workers.push(worker),
            Err(error) => {
                self.pending = false;
                self.status = format!("Cannot start find worker: {error}");
            }
        }
    }

    pub fn poll(&mut self, mut tab: Option<&mut crate::editor::EditorTab>) {
        while let Ok(completion) = self.receiver.try_recv() {
            if completion.generation != self.generation {
                continue;
            }
            self.pending = false;
            let Some(tab) = tab.as_deref_mut() else {
                self.status = "The active file changed before search finished".into();
                continue;
            };
            if tab.path != completion.path || tab.document.revision != completion.revision {
                self.status = "The file changed before search finished; try again".into();
                continue;
            }
            match completion.result {
                FindResult::Search(Some((start, end, wrapped)), previous) => {
                    tab.document.set_cursor(if previous { start } else { end });
                    tab.select_char_range(start, end);
                    self.last_match = Some((PathBufKey(tab.path.clone()), start, end));
                    self.status = if wrapped {
                        "Wrapped to the other end of the file".into()
                    } else {
                        "Match selected".into()
                    };
                }
                FindResult::Search(None, _) => {
                    self.last_match = None;
                    self.status = "No match in this file".into();
                }
                FindResult::ReplaceOne(Some((text, start, end))) => {
                    tab.document.text = text;
                    tab.document.mark_changed();
                    tab.document.set_cursor(end);
                    tab.select_char_range(start, end);
                    self.last_match = None;
                    self.status = "Replaced one match".into();
                }
                FindResult::ReplaceOne(None) => self.status = "No match in this file".into(),
                FindResult::ReplaceAll(text, count, first_start) if count > 0 => {
                    tab.document.text = text;
                    tab.document.mark_changed();
                    let start = first_start.unwrap_or(0);
                    let end = start + self.replacement.chars().count();
                    tab.document.set_cursor(end);
                    tab.select_char_range(start, end);
                    self.last_match = None;
                    self.status = format!("Replaced {count} matches");
                }
                FindResult::ReplaceAll(_, _, _) => self.status = "No match in this file".into(),
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
}

fn find_range(
    text: &str,
    query: &str,
    case_sensitive: bool,
    cursor: usize,
    previous: bool,
) -> Option<(usize, usize, bool)> {
    if query.is_empty() {
        return None;
    }
    let query_chars = query.chars().count();
    let mut first = None;
    let mut last = None;
    let mut before = None;
    let mut after = None;
    for (start, (byte, _)) in text.char_indices().enumerate() {
        let Some(candidate) = text.get(byte..byte.saturating_add(query.len())) else {
            continue;
        };
        let matched = if case_sensitive {
            candidate == query
        } else {
            candidate.eq_ignore_ascii_case(query)
        };
        if !matched {
            continue;
        }
        let range = (start, start + query_chars);
        first.get_or_insert(range);
        last = Some(range);
        if range.1 <= cursor {
            before = Some(range);
        }
        if range.0 >= cursor && after.is_none() {
            after = Some(range);
        }
    }
    let selected = if previous {
        before
            .map(|range| (range, false))
            .or_else(|| last.map(|range| (range, true)))
    } else {
        after
            .map(|range| (range, false))
            .or_else(|| first.map(|range| (range, true)))
    };
    selected.map(|((start, end), wrapped)| (start, end, wrapped))
}

fn replace_all_text(
    text: &str,
    query: &str,
    replacement: &str,
    case_sensitive: bool,
) -> (String, usize, Option<usize>) {
    if query.is_empty() {
        return (text.to_owned(), 0, None);
    }
    let mut output = String::with_capacity(text.len());
    let mut cursor_byte = 0;
    let mut count = 0;
    let mut first_start = None;
    for (start_char, (start_byte, _)) in text.char_indices().enumerate() {
        if start_byte < cursor_byte {
            continue;
        }
        let Some(candidate) = text.get(start_byte..start_byte.saturating_add(query.len())) else {
            continue;
        };
        let matched = if case_sensitive {
            candidate == query
        } else {
            candidate.eq_ignore_ascii_case(query)
        };
        if matched {
            let end_byte = start_byte + query.len();
            output.push_str(&text[cursor_byte..start_byte]);
            output.push_str(replacement);
            cursor_byte = end_byte;
            first_start.get_or_insert(start_char);
            count += 1;
        }
    }
    output.push_str(&text[cursor_byte..]);
    (output, count, first_start)
}

fn char_range_matches(
    text: &str,
    start: usize,
    end: usize,
    query: &str,
    case_sensitive: bool,
) -> bool {
    let Some(byte_start) = text
        .char_indices()
        .nth(start)
        .map(|(byte, _)| byte)
        .or_else(|| (start == text.chars().count()).then_some(text.len()))
    else {
        return false;
    };
    let Some(byte_end) = text
        .char_indices()
        .nth(end)
        .map(|(byte, _)| byte)
        .or_else(|| (end == text.chars().count()).then_some(text.len()))
    else {
        return false;
    };
    text.get(byte_start..byte_end).is_some_and(|candidate| {
        if case_sensitive {
            candidate == query
        } else {
            candidate.eq_ignore_ascii_case(query)
        }
    })
}

fn replace_char_range(text: &mut String, start: usize, end: usize, replacement: &str) {
    let byte_start = text
        .char_indices()
        .nth(start)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len());
    let byte_end = text
        .char_indices()
        .nth(end)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len());
    text.replace_range(byte_start..byte_end, replacement);
}
