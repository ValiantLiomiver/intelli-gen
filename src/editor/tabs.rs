use std::path::PathBuf;

use eframe::egui;

use super::{Document, highlighter::Highlighter};
use super::document::{EDITOR_PAGE_BYTES, PAGED_EDITOR_BYTES};
use crate::app::{AppState, events::AppEvent};

pub struct EditorTab {
    /// Canonical identity, matching the loaded document's path.
    pub path: PathBuf,
    pub document: Document,
    pub saving: bool,
    pub save_again: bool,
    pub close_after_save: bool,
    pub pending_rename_to: Option<PathBuf>,
    highlighter: Highlighter,
    pending_line: Option<usize>,
    pending_character_index: Option<usize>,
    pending_selection: Option<(usize, usize)>,
    page_start: usize,
    page_end: usize,
    page_cursor: Option<usize>,
}

impl EditorTab {
    pub fn new(document: Document) -> Self {
        Self {
            path: document.path.clone(),
            document,
            saving: false,
            save_again: false,
            close_after_save: false,
            pending_rename_to: None,
            highlighter: Highlighter::default(),
            pending_line: None,
            pending_character_index: None,
            pending_selection: None,
            page_start: 0,
            page_end: 0,
            page_cursor: None,
        }
    }

    pub fn navigate_to_line(&mut self, line: usize) {
        self.pending_line = Some(line.max(1));
    }

    pub fn navigate_to_character_index(&mut self, index: usize) {
        self.pending_line = None;
        self.pending_character_index = Some(index.min(self.document.text.chars().count()));
    }

    pub fn restore_document_cursor(&mut self) {
        self.pending_character_index = Some(self.document.character_index());
    }

    pub fn refresh_after_workspace_edit(&mut self) {
        self.page_start = 0;
        self.page_end = 0;
        self.page_cursor = None;
        self.pending_character_index = Some(self.document.character_index());
        self.highlighter = Highlighter::default();
    }

    pub fn select_char_range(&mut self, start: usize, end: usize) {
        if self.document.text.len() > PAGED_EDITOR_BYTES {
            self.pending_character_index = Some(start);
            self.pending_selection = None;
        } else {
            self.pending_selection = Some((start, end));
            self.pending_character_index = None;
        }
    }

    fn render_editor(
        &mut self,
        ui: &mut egui::Ui,
        allow_edits: bool,
        breakpoints: &[u32],
        current_line: Option<u32>,
        diagnostics: &[crate::java::lsp::LanguageDiagnostic],
    ) -> Option<u32> {
        if self.document.text.len() > PAGED_EDITOR_BYTES {
            return self.render_paged_editor(
                ui,
                allow_edits && !self.close_after_save,
                breakpoints,
                current_line,
                diagnostics,
            );
        }
        self.page_end = 0;
        let mut toggled = None;
        let editor_id = egui::Id::new(("source_editor", &self.path));
        if let Some((start, end)) = self.pending_selection.take() {
            let mut state = egui::text_edit::TextEditState::load(ui.ctx(), editor_id).unwrap_or_default();
            state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(start),
                egui::text::CCursor::new(end),
            )));
            state.store(ui.ctx(), editor_id);
            ui.memory_mut(|memory| memory.request_focus(editor_id));
        }
        if let Some(index) = self.pending_character_index.take() {
            let mut state = egui::text_edit::TextEditState::load(ui.ctx(), editor_id).unwrap_or_default();
            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(index))));
            state.store(ui.ctx(), editor_id);
            ui.memory_mut(|memory| memory.request_focus(editor_id));
        }
        let navigation = self.pending_line.take().map(|line| {
            let line = line.min(self.document.text.lines().count().max(1));
            let index = self
                .document
                .text
                .split_inclusive('\n')
                .take(line - 1)
                .map(|text| text.chars().count())
                .sum::<usize>();
            let mut state =
                egui::text_edit::TextEditState::load(ui.ctx(), editor_id).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(index),
                )));
            state.store(ui.ctx(), editor_id);
            ui.memory_mut(|memory| memory.request_focus(editor_id));
            self.document.set_cursor(index);
            line
        });
        let language = super::highlighter::SourceLanguage::for_path(&self.path);
        let java = language == super::highlighter::SourceLanguage::Java;
        let highlighter = &mut self.highlighter;
        let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, _width: f32| {
            let mut job = highlighter.layout(ui, buffer.as_str(), language);
            job.wrap.max_width = f32::INFINITY;
            ui.fonts_mut(|fonts| fonts.layout_job(job))
        };
        egui::ScrollArea::both()
            .id_salt(("source_scroll", &self.path))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.allocate_exact_size(egui::vec2(58.0, 0.0), egui::Sense::hover());
                let output = egui::TextEdit::multiline(&mut self.document.text)
                    .id(editor_id)
                    .code_editor()
                    .font(egui::FontId::monospace(14.0))
                    .desired_width(f32::INFINITY)
                    .min_size(ui.available_size())
                    .cursor_at_end(false)
                    .interactive(allow_edits && !self.close_after_save)
                    .layouter(&mut layouter)
                    .show(ui);
                if output.response.changed() {
                    self.document.mark_changed();
                }
                toggled = super::editor_gutter(
                    ui,
                    &output,
                    &self.path,
                    1,
                    java.then_some(breakpoints),
                    java.then_some(current_line).flatten(),
                    diagnostics,
                );
                if let Some(line) = navigation {
                    let height =
                        ui.fonts_mut(|fonts| fonts.row_height(&egui::FontId::monospace(14.0)));
                    let target = egui::Rect::from_min_size(
                        output.response.rect.min + egui::vec2(0.0, (line - 1) as f32 * height),
                        egui::vec2(1.0, height),
                    );
                    ui.scroll_to_rect(target, Some(egui::Align::Center));
                }
                if let Some(cursor) = output.cursor_range {
                    self.document.set_cursor(cursor.primary.index.into());
                }
                });
            });
        toggled
    }

    /// Edit a bounded page while retaining the complete document in Document::text.
    fn render_paged_editor(
        &mut self,
        ui: &mut egui::Ui,
        allow_edits: bool,
        breakpoints: &[u32],
        current_line: Option<u32>,
        diagnostics: &[crate::java::lsp::LanguageDiagnostic],
    ) -> Option<u32> {
        ui.colored_label(ui.visuals().warn_fg_color,
            "Large file: editing is limited to the visible page. Save writes the complete document.");
        let mut start = None;
        if let Some(line) = self.pending_line.take() {
            start = Some(self.document.text.split_inclusive('\n').take(line.saturating_sub(1)).map(str::len).sum());
        }
        if let Some(index) = self.pending_character_index.take() {
            start = Some(self.document.text.char_indices().nth(index).map_or(self.document.text.len(), |(byte, _)| byte));
        }
        ui.horizontal(|ui| {
            if ui.add_enabled(self.page_start > 0, egui::Button::new("Previous page")).clicked() {
                start = Some(self.page_start.saturating_sub(EDITOR_PAGE_BYTES));
            }
            if ui.add_enabled(self.page_end < self.document.text.len(), egui::Button::new("Next page")).clicked() {
                start = Some(self.page_end);
            }
            ui.small(format!("Bytes {}–{} of {}", self.page_start, self.page_end, self.document.text.len()));
        });
        if start.is_none() && self.page_end == 0 {
            let index = self.document.character_index();
            start = Some(self.document.text.char_indices().nth(index).map_or(self.document.text.len(), |(byte, _)| byte));
        }
        let reset = start.is_some() || self.page_end == 0 || self.page_start >= self.document.text.len();
        if reset {
            self.page_start = start.unwrap_or(0).min(self.document.text.len());
            while !self.document.text.is_char_boundary(self.page_start) { self.page_start -= 1; }
            self.page_end = (self.page_start + EDITOR_PAGE_BYTES).min(self.document.text.len());
            while !self.document.text.is_char_boundary(self.page_end) { self.page_end -= 1; }
            // Prefer a complete logical line, but also bound huge single lines.
            if self.page_end < self.document.text.len() {
                if let Some(newline) = self.document.text[self.page_start..self.page_end].rfind('\n') {
                    self.page_end = self.page_start + newline + 1;
                }
            }
            self.page_cursor = None;
        }
        // Server edits can change byte boundaries between frames.
        self.page_start = self.page_start.min(self.document.text.len());
        while !self.document.text.is_char_boundary(self.page_start) { self.page_start -= 1; }
        self.page_end = self.page_end.min(self.document.text.len()).min(self.page_start + EDITOR_PAGE_BYTES);
        while !self.document.text.is_char_boundary(self.page_end) { self.page_end -= 1; }
        let editor_id = egui::Id::new(("paged_source_editor", &self.path));
        if reset {
            egui::text_edit::TextEditState::default().store(ui.ctx(), editor_id);
            ui.memory_mut(|memory| memory.request_focus(editor_id));
        }
        let first_line = self.document.text[..self.page_start]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count() as u32 + 1;
        let mut page = self.document.text[self.page_start..self.page_end].to_owned();
        let mut toggled = None;
        let highlighter = &mut self.highlighter;
        let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, _width: f32| {
            let mut job = highlighter.layout(
                ui,
                buffer.as_str(),
                super::highlighter::SourceLanguage::for_path(&self.path),
            );
            job.wrap.max_width = f32::INFINITY;
            ui.fonts_mut(|fonts| fonts.layout_job(job))
        };
        let output = egui::ScrollArea::both().id_salt(("paged_source_scroll", &self.path, self.page_start))
            .auto_shrink([false, false]).show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.allocate_exact_size(egui::vec2(58.0, 0.0), egui::Sense::hover());
                    let output = egui::TextEdit::multiline(&mut page).id(editor_id).code_editor()
                        .font(egui::FontId::monospace(14.0)).desired_width(f32::INFINITY)
                        .min_size(ui.available_size()).cursor_at_end(false)
                        .interactive(allow_edits)
                        .layouter(&mut layouter).show(ui);
                    toggled = super::editor_gutter(
                        ui, &output, &self.path, first_line, Some(breakpoints), current_line, diagnostics,
                    );
                    output
                })
            }).inner.inner;
        if output.response.changed() {
            self.document.text.replace_range(self.page_start..self.page_end, &page);
            self.page_end = self.page_start + page.len();
            self.document.mark_changed();
            self.page_cursor = None;
        }
        if let Some(cursor) = output.cursor_range {
            let page_cursor: usize = cursor.primary.index.into();
            if self.page_cursor != Some(page_cursor) {
                let index = self.document.text[..self.page_start].chars().count() + page_cursor;
                self.document.set_cursor(index);
                self.page_cursor = Some(page_cursor);
            }
        }
        toggled
    }
}

pub fn render(ui: &mut egui::Ui, state: &mut AppState) {
    let mut close = None;
    egui::ScrollArea::horizontal()
        .id_salt("editor_tabs")
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for (index, tab) in state.editor_tabs.iter().enumerate() {
                    let name = tab
                        .path
                        .file_name()
                        .unwrap_or(tab.path.as_os_str())
                        .to_string_lossy();
                    let duplicate_name = state
                        .editor_tabs
                        .iter()
                        .enumerate()
                        .any(|(other_index, other)| {
                            other_index != index
                                && other.path.file_name() == tab.path.file_name()
                        });
                    let display_name = if duplicate_name {
                        let location = state
                            .project
                            .as_ref()
                            .and_then(|project| tab.path.strip_prefix(&project.root).ok())
                            .unwrap_or(&tab.path);
                        let parent = location
                            .parent()
                            .filter(|parent| !parent.as_os_str().is_empty());
                        parent.map_or_else(
                            || name.to_string(),
                            |parent| format!("{name} — {}", parent.display()),
                        )
                    } else {
                        name.to_string()
                    };
                    let label = format!(
                        "{}{}{}",
                        display_name,
                        if tab.document.is_dirty() { " •" } else { "" },
                        if tab.saving { " (saving…)" } else { "" }
                    );
                    let active = state.active_tab == Some(index);
                    let path = tab.path.clone();
                    let frame = egui::Frame::new()
                        .fill(if active {
                            ui.visuals().selection.bg_fill
                        } else {
                            ui.visuals().widgets.inactive.bg_fill
                        })
                        .corner_radius(egui::CornerRadius::same(4))
                        .inner_margin(egui::Margin::symmetric(4, 2));
                    frame.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            let response = ui
                                .selectable_label(active, label)
                                .on_hover_text(path.display().to_string());
                            if response.clicked() {
                                state.active_tab = Some(index);
                            }
                            if response.middle_clicked() {
                                close = Some(path.clone());
                            }
                            if ui.small_button("×").on_hover_text("Close file").clicked() {
                                close = Some(path);
                            }
                        });
                    });
                }
            });
        });
    if let Some(path) = close {
        state.enqueue(AppEvent::CloseFile(path));
    }
    ui.separator();
    crate::java::lsp::render(ui, state);
    let active_path = state.active_tab.and_then(|index| state.editor_tabs.get(index)).map(|tab| tab.path.clone());
    let diagnostics = active_path.as_ref()
        .and_then(|path| state.language.diagnostics.get(path))
        .map_or(&[][..], Vec::as_slice);
    if let Some(tab) = state
        .active_tab
        .and_then(|index| state.editor_tabs.get_mut(index))
    {
        ui.label(tab.path.display().to_string());
        ui.separator();
        let lines = state.debugger.breakpoints.lines(&tab.path);
        let current_line = state.debugger.inspector.selected_frame.and_then(|id| state.debugger.inspector.frames.iter().find(|frame| frame.id == id))
            .filter(|frame| frame.source.as_ref() == Some(&tab.path)).map(|frame| frame.line);
        let toggled = tab.render_editor(ui, !state.quit_approved, &lines, current_line, diagnostics);
        let path = tab.path.clone();
        let modified = tab.document.is_dirty();
        if modified { state.debugger.breakpoints.modified.insert(path.clone()); }
        else { state.debugger.breakpoints.modified.remove(&path); }
        if let Some(line) = toggled {
            state.enqueue(AppEvent::Debugger(crate::app::events::DebuggerAction::ToggleBreakpoint { source: path, line, modified }));
        }
    }
}

pub fn close_prompt(context: &egui::Context, state: &mut AppState) {
    let Some(path) = state.pending_close.clone() else {
        return;
    };
    let Some(tab) = state.editor_tabs.iter().find(|tab| tab.path == path) else {
        state.pending_close = None;
        return;
    };
    let saving = tab.saving;
    let closing = tab.close_after_save;
    let mut decision = None;
    egui::Modal::new(egui::Id::new("unsaved_document")).show(context, |ui| {
        ui.heading("Save changes before closing?");
        ui.label(path.display().to_string());
        ui.label(if saving {
            "Waiting for the current save…"
        } else {
            "This file contains unsaved changes."
        });
        if let Some(error) = &state.last_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!saving && !closing, egui::Button::new("Save and Close"))
                .clicked()
            {
                decision = Some(AppEvent::SaveAndClose(path.clone()));
            }
            if ui
                .add_enabled(!saving && !closing, egui::Button::new("Discard Changes"))
                .clicked()
            {
                decision = Some(AppEvent::DiscardAndClose(path.clone()));
            }
            if ui
                .add_enabled(!closing, egui::Button::new("Cancel"))
                .clicked()
            {
                decision = Some(AppEvent::CancelClose);
            }
        });
    });
    if let Some(event) = decision {
        state.enqueue(event);
    }
}
