use std::path::PathBuf;

use eframe::egui;

use super::{Document, highlighter::Highlighter};
use super::document::{EDITOR_PAGE_BYTES, PAGED_EDITOR_BYTES};
use crate::app::{AppState, events::AppEvent};

fn byte_index(text: &str, character_index: usize) -> usize {
    text.char_indices()
        .nth(character_index)
        .map_or(text.len(), |(byte, _)| byte)
}

fn toggle_line_comment(text: &mut String, start: usize, end: usize) -> Option<(usize, usize)> {
    let char_count = text.chars().count();
    let start_byte = byte_index(text, start.min(char_count));
    let end_byte = byte_index(text, end.min(char_count));
    let last_byte = if start_byte < end_byte && text[..end_byte].ends_with('\n') {
        end_byte - 1
    } else {
        end_byte
    };
    let first_line = text[..start_byte].rfind('\n').map_or(0, |newline| newline + 1);
    let last_line = text[..last_byte].rfind('\n').map_or(0, |newline| newline + 1);
    let mut line_starts = Vec::new();
    let mut line_start = first_line;
    loop {
        line_starts.push(line_start);
        if line_start >= last_line { break; }
        let Some(newline) = text[line_start..].find('\n') else { break; };
        line_start += newline + 1;
    }
    let lines: Vec<_> = line_starts.iter().map(|line_start| {
        let line_end = text[*line_start..].find('\n').map_or(text.len(), |newline| *line_start + newline);
        let content_end = if text.as_bytes().get(line_end.wrapping_sub(1)) == Some(&b'\r') { line_end - 1 } else { line_end };
        let indent_end = *line_start + text[*line_start..content_end].bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t')).count();
        (*line_start, content_end, indent_end)
    }).collect();
    let nonblank: Vec<_> = lines.iter().filter(|(start, end, _)| !text[*start..*end].trim().is_empty()).collect();
    let all_commented = !nonblank.is_empty() && nonblank.iter().all(|(_line_start, line_end, indent_end)| {
        text[*indent_end..*line_end].starts_with("//")
    });
    let mut edits = Vec::new();
    for (_, line_end, indent_end) in &lines {
        if all_commented {
            let content = &text[*indent_end..*line_end];
            if content.starts_with("//") {
                let remove_end = if content.as_bytes().get(2) == Some(&b' ') { indent_end + 3 } else { indent_end + 2 };
                edits.push((*indent_end, remove_end, String::new()));
            }
        } else {
            let line_is_blank = text[*indent_end..*line_end].trim().is_empty();
            edits.push((*indent_end, *indent_end, if line_is_blank { "//".into() } else { "// ".into() }));
        }
    }
    if edits.is_empty() { return None; }
    let map_position = |position: usize| {
        let mut shift: isize = 0;
        for (edit_start, edit_end, replacement) in &edits {
            if position < *edit_start { break; }
            if position < *edit_end {
                return (*edit_start as isize + shift + replacement.len() as isize).max(0) as usize;
            }
            shift += replacement.len() as isize - (*edit_end - *edit_start) as isize;
        }
        (position as isize + shift).max(0) as usize
    };
    let mapped_start = map_position(start_byte);
    let mapped_end = map_position(end_byte);
    for (edit_start, edit_end, replacement) in edits.into_iter().rev() {
        text.replace_range(edit_start..edit_end, &replacement);
    }
    Some((
        text[..mapped_start.min(text.len())].chars().count(),
        text[..mapped_end.min(text.len())].chars().count(),
    ))
}

fn indent_java_lines(text: &mut String, start: usize, end: usize, outdent: bool, unit: &str, indent_size: usize) -> Option<(usize, usize)> {
    let char_count = text.chars().count();
    let start_byte = byte_index(text, start.min(char_count));
    let end_byte = byte_index(text, end.min(char_count));
    let last_byte = if start_byte < end_byte && text[..end_byte].ends_with('\n') { end_byte - 1 } else { end_byte };
    let first_line = text[..start_byte].rfind('\n').map_or(0, |newline| newline + 1);
    let last_line = text[..last_byte].rfind('\n').map_or(0, |newline| newline + 1);
    let mut edits = Vec::new();
    let mut line_start = first_line;
    loop {
        if outdent {
            let remove = if text.as_bytes().get(line_start) == Some(&b'\t') {
                1
            } else {
                text[line_start..].bytes().take(indent_size).take_while(|byte| *byte == b' ').count()
            };
            if remove > 0 { edits.push((line_start, line_start + remove, String::new())); }
        } else {
            edits.push((line_start, line_start, unit.to_owned()));
        }
        if line_start >= last_line { break; }
        let Some(newline) = text[line_start..].find('\n') else { break; };
        line_start += newline + 1;
    }
    if edits.is_empty() { return None; }
    let map_position = |position: usize| {
        let mut shift: isize = 0;
        for (edit_start, edit_end, replacement) in &edits {
            if position < *edit_start { break; }
            if position < *edit_end {
                return (*edit_start as isize + shift + replacement.len() as isize).max(0) as usize;
            }
            shift += replacement.len() as isize - (*edit_end - *edit_start) as isize;
        }
        (position as isize + shift).max(0) as usize
    };
    let mapped_start = map_position(start_byte);
    let mapped_end = map_position(end_byte);
    for (edit_start, edit_end, replacement) in edits.into_iter().rev() {
        text.replace_range(edit_start..edit_end, &replacement);
    }
    Some((
        text[..mapped_start.min(text.len())].chars().count(),
        text[..mapped_end.min(text.len())].chars().count(),
    ))
}

fn java_completion_trigger(document: &Document, events: &[egui::Event]) -> Option<crate::java::lsp::EditorCommand> {
    let text_events = events.iter().filter_map(|event| match event {
        egui::Event::Text(text) | egui::Event::Paste(text) => Some(text.as_str()),
        _ => None,
    });
    let mut typed_identifier = false;
    let mut typed_dot = false;
    for text in text_events {
        typed_dot |= text == ".";
        typed_identifier |= text.chars().any(|character| {
            character.is_alphanumeric() || matches!(character, '_' | '$')
        });
    }
    if typed_dot {
        return Some(crate::java::lsp::EditorCommand::Completion);
    }
    if !typed_identifier {
        return None;
    }
    let caret = document.character_index().min(document.text.chars().count());
    let caret_byte = byte_index(&document.text, caret);
    let identifier_prefix = document.text[..caret_byte].chars().rev().take_while(|character| {
        character.is_alphanumeric() || matches!(character, '_' | '$')
    }).count();
    (identifier_prefix >= 2).then_some(crate::java::lsp::EditorCommand::Completion)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum JavaPairEdit {
    None,
    SkippedClosing,
    InsertedPair,
}

fn auto_delete_java_pair(ui: &egui::Ui, text: &mut String, editor_id: egui::Id) -> bool {
    if ui.memory(|memory| memory.focused()) != Some(editor_id) {
        return false;
    }
    let Some(mut state) = egui::text_edit::TextEditState::load(ui.ctx(), editor_id) else {
        return false;
    };
    let Some(range) = state.cursor.char_range() else { return false; };
    if range.primary != range.secondary {
        return false;
    }
    let caret: usize = range.primary.index.into();
    if caret == 0 || caret >= text.chars().count() {
        return false;
    }
    let caret_byte = byte_index(text, caret);
    let Some(previous) = text[..caret_byte].chars().next_back() else { return false; };
    let Some(next) = text[caret_byte..].chars().next() else { return false; };
    if !matches!((previous, next), ('(', ')') | ('{', '}') | ('[', ']') | ('"', '"') | ('\'', '\'')) {
        return false;
    }
    if !ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)) {
        return false;
    }
    let start = caret_byte - previous.len_utf8();
    let end = caret_byte + next.len_utf8();
    text.replace_range(start..end, "");
    state.cursor.set_char_range(Some(egui::text::CCursorRange::one(
        egui::text::CCursor::new(caret - 1),
    )));
    state.store(ui.ctx(), editor_id);
    true
}

fn auto_pair_java_character(ui: &egui::Ui, text: &mut String, editor_id: egui::Id) -> JavaPairEdit {
    if ui.memory(|memory| memory.focused()) != Some(editor_id) {
        return JavaPairEdit::None;
    }
    let Some(mut state) = egui::text_edit::TextEditState::load(ui.ctx(), editor_id) else {
        return JavaPairEdit::None;
    };
    let Some(range) = state.cursor.char_range() else {
        return JavaPairEdit::None;
    };
    let primary: usize = range.primary.index.into();
    let secondary: usize = range.secondary.index.into();
    let has_selection = primary != secondary;
    let typed = ui.input_mut(|input| {
        let index = input.events.iter().position(|event| {
            matches!(event, egui::Event::Text(character)
                if if has_selection {
                    matches!(character.as_str(), "(" | "{" | "[" | "\"" | "'")
                } else {
                    matches!(character.as_str(), "(" | "{" | "[" | ")" | "}" | "]" | "\"" | "'")
                })
        })?;
        match input.events.remove(index) {
            egui::Event::Text(character) => Some(character),
            _ => None,
        }
    });
    let Some(typed) = typed else { return JavaPairEdit::None; };
    let close = match typed.as_str() {
        "(" => Some(")"),
        "{" => Some("}"),
        "[" => Some("]"),
        "\"" => Some("\""),
        "'" => Some("'"),
        _ => None,
    };
    if has_selection {
        let Some(close) = close else { return JavaPairEdit::None; };
        let start = primary.min(secondary);
        let end = primary.max(secondary);
        let start_byte = byte_index(text, start);
        let end_byte = byte_index(text, end);
        text.insert_str(end_byte, close);
        text.insert_str(start_byte, &typed);
        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(primary + 1),
            egui::text::CCursor::new(secondary + 1),
        )));
        state.store(ui.ctx(), editor_id);
        return JavaPairEdit::InsertedPair;
    }
    let caret = primary;
    if close.is_none() {
        let caret_byte = byte_index(text, caret);
        if text[caret_byte..].starts_with(&typed) {
            let next = caret + 1;
            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(next),
            )));
            state.store(ui.ctx(), editor_id);
            return JavaPairEdit::SkippedClosing;
        }
        text.insert_str(caret_byte, &typed);
        let next = caret + 1;
        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(
            egui::text::CCursor::new(next),
        )));
        state.store(ui.ctx(), editor_id);
        return JavaPairEdit::InsertedPair;
    }
    let caret_byte = byte_index(text, caret);
    let mut insertion = typed;
    insertion.push_str(close.unwrap_or_default());
    text.insert_str(caret_byte, &insertion);
    let next = caret + 1;
    state.cursor.set_char_range(Some(egui::text::CCursorRange::one(
        egui::text::CCursor::new(next),
    )));
    state.store(ui.ctx(), editor_id);
    JavaPairEdit::InsertedPair
}

fn auto_indent_java_enter(ui: &egui::Ui, text: &mut String, editor_id: egui::Id, indent_size: usize, insert_spaces: bool) -> bool {
    if ui.memory(|memory| memory.focused()) != Some(editor_id) {
        return false;
    }
    let Some(mut state) = egui::text_edit::TextEditState::load(ui.ctx(), editor_id) else {
        return false;
    };
    let Some(range) = state.cursor.char_range() else {
        return false;
    };
    if !ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
        return false;
    }
    let primary: usize = range.primary.index.into();
    let secondary: usize = range.secondary.index.into();
    let start = primary.min(secondary);
    let end = primary.max(secondary);
    let start_byte = byte_index(text, start);
    let end_byte = byte_index(text, end);
    let line_start = text[..start_byte].rfind('\n').map_or(0, |newline| newline + 1);
    let prefix = &text[line_start..start_byte];
    let indent: String = prefix
        .chars()
        .take_while(|character| matches!(character, ' ' | '\t'))
        .collect();
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let opens_block = prefix.trim_end().ends_with('{');
    let indent_unit = if opens_block {
        if insert_spaces { " ".repeat(indent_size) } else { "\t".to_owned() }
    } else {
        String::new()
    };
    let mut replace_end = end_byte;
    let mut insertion = format!("{newline}{indent}{indent_unit}");
    let cursor_byte = insertion.len();
    if start == end && opens_block {
        let suffix = &text[end_byte..];
        let close_offset = suffix.bytes().take_while(|byte| matches!(byte, b' ' | b'\t')).count();
        if suffix.as_bytes().get(close_offset) == Some(&b'}') {
            replace_end = end_byte + close_offset;
            insertion.push_str(newline);
            insertion.push_str(&indent);
        }
    }
    text.replace_range(start_byte..replace_end, &insertion);
    let cursor = text[..start_byte + cursor_byte].chars().count();
    state.cursor.set_char_range(Some(egui::text::CCursorRange::one(
        egui::text::CCursor::new(cursor),
    )));
    state.store(ui.ctx(), editor_id);
    true
}

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
    snippet_tab_stops: Vec<(u32, Vec<(usize, usize)>)>,
    active_snippet_tab_stop: Option<usize>,
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
            snippet_tab_stops: Vec::new(),
            active_snippet_tab_stop: None,
            page_start: 0,
            page_end: 0,
            page_cursor: None,
        }
    }

    pub fn navigate_to_line(&mut self, line: usize) {
        self.active_snippet_tab_stop = None;
        self.snippet_tab_stops.clear();
        self.pending_selection = None;
        self.pending_line = Some(line.max(1));
    }

    pub fn navigate_to_character_index(&mut self, index: usize) {
        self.active_snippet_tab_stop = None;
        self.snippet_tab_stops.clear();
        self.pending_selection = None;
        self.pending_line = None;
        self.pending_character_index = Some(index.min(self.document.text.chars().count()));
    }

    pub fn restore_document_cursor(&mut self) {
        self.active_snippet_tab_stop = None;
        self.snippet_tab_stops.clear();
        self.pending_selection = None;
        self.pending_character_index = Some(self.document.character_index());
    }

    pub fn set_snippet_tab_stops(&mut self, stops: Vec<(u32, Vec<(usize, usize)>)>) {
        if stops.is_empty() {
            self.restore_document_cursor();
            return;
        }
        if self.document.text.len() > PAGED_EDITOR_BYTES {
            self.active_snippet_tab_stop = None;
            self.snippet_tab_stops.clear();
            self.pending_selection = None;
            self.pending_character_index = Some(stops[0].1[0].0);
            return;
        }
        self.active_snippet_tab_stop = Some(0);
        self.pending_selection = Some(stops[0].1[0]);
        self.pending_character_index = None;
        self.snippet_tab_stops = stops;
    }

    pub fn has_snippet_tab_stops(&self) -> bool {
        self.active_snippet_tab_stop.is_some()
    }

    pub fn advance_snippet_tab_stop(&mut self, backwards: bool) -> bool {
        let Some(current) = self.active_snippet_tab_stop else { return false; };
        self.sync_active_snippet_mirrors();
        let next = if backwards {
            current.checked_sub(1)
        } else {
            Some(current + 1).filter(|index| *index < self.snippet_tab_stops.len())
        };
        match next {
            Some(index) => {
                self.active_snippet_tab_stop = Some(index);
                self.pending_selection = Some(self.snippet_tab_stops[index].1[0]);
                self.pending_character_index = None;
            }
            None if !backwards => {
                self.active_snippet_tab_stop = None;
                self.snippet_tab_stops.clear();
                self.pending_selection = None;
            }
            None => {}
        }
        true
    }

    pub fn toggle_java_line_comment(&mut self, context: &egui::Context) -> bool {
        if self.close_after_save || !self.path.extension().is_some_and(|extension| extension == "java") {
            return false;
        }
        let paged = self.document.text.len() > PAGED_EDITOR_BYTES;
        let editor_id = if paged {
            egui::Id::new(("paged_source_editor", &self.path))
        } else {
            egui::Id::new(("source_editor", &self.path))
        };
        if context.memory(|memory| memory.focused()) != Some(editor_id) {
            return false;
        }
        self.active_snippet_tab_stop = None;
        self.snippet_tab_stops.clear();
        let Some(mut state) = egui::text_edit::TextEditState::load(context, editor_id) else {
            return false;
        };
        let Some(range) = state.cursor.char_range() else { return false; };
        let base = if paged { self.document.text[..self.page_start].chars().count() } else { 0 };
        let primary = base + usize::from(range.primary.index);
        let secondary = base + usize::from(range.secondary.index);
        let Some((primary, secondary)) = toggle_line_comment(&mut self.document.text, primary, secondary) else {
            return false;
        };
        self.document.mark_changed();
        if paged {
            let target = (self.page_start + EDITOR_PAGE_BYTES).min(self.document.text.len());
            self.page_end = if target == self.document.text.len() {
                target
            } else {
                self.document.text[..target].rfind('\n').map_or(target, |newline| newline + 1)
            };
            while !self.document.text.is_char_boundary(self.page_end) { self.page_end -= 1; }
        }
        let base = if paged { self.document.text[..self.page_start].chars().count() } else { 0 };
        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(primary.saturating_sub(base)),
            egui::text::CCursor::new(secondary.saturating_sub(base)),
        )));
        state.store(context, editor_id);
        self.page_cursor = None;
        true
    }

    pub fn indent_java_selection(&mut self, context: &egui::Context, outdent: bool, indent_size: usize, insert_spaces: bool) -> bool {
        if self.close_after_save || !self.path.extension().is_some_and(|extension| extension == "java") {
            return false;
        }
        let paged = self.document.text.len() > PAGED_EDITOR_BYTES;
        let editor_id = if paged {
            egui::Id::new(("paged_source_editor", &self.path))
        } else {
            egui::Id::new(("source_editor", &self.path))
        };
        if context.memory(|memory| memory.focused()) != Some(editor_id) { return false; }
        self.active_snippet_tab_stop = None;
        self.snippet_tab_stops.clear();
        let Some(mut state) = egui::text_edit::TextEditState::load(context, editor_id) else { return false; };
        let Some(range) = state.cursor.char_range() else { return false; };
        let base = if paged { self.document.text[..self.page_start].chars().count() } else { 0 };
        let primary_local: usize = range.primary.index.into();
        let secondary_local: usize = range.secondary.index.into();
        let (primary, secondary) = if primary_local != secondary_local {
            let primary = base + primary_local;
            let secondary = base + secondary_local;
            let unit = if insert_spaces { " ".repeat(indent_size) } else { "\t".to_owned() };
            let Some(mapped) = indent_java_lines(&mut self.document.text, primary, secondary, outdent, &unit, indent_size) else {
                return false;
            };
            mapped
        } else {
            let caret = base + primary_local;
            let caret_byte = byte_index(&self.document.text, caret);
            let new_caret = if outdent {
                let line_start = self.document.text[..caret_byte].rfind('\n').map_or(0, |newline| newline + 1);
                let remove = if self.document.text[line_start..].starts_with('\t') {
                    1
                } else if self.document.text[line_start..].starts_with(&" ".repeat(indent_size)) {
                    indent_size
                } else {
                    return false;
                };
                self.document.text.replace_range(line_start..line_start + remove, "");
                if caret_byte >= line_start + remove { caret - remove } else { line_start }
            } else {
                let unit = if insert_spaces { " ".repeat(indent_size) } else { "\t".to_owned() };
                self.document.text.insert_str(caret_byte, &unit);
                caret + unit.chars().count()
            };
            (new_caret, new_caret)
        };
        self.document.mark_changed();
        if paged {
            let target = (self.page_start + EDITOR_PAGE_BYTES).min(self.document.text.len());
            self.page_end = if target == self.document.text.len() {
                target
            } else {
                self.document.text[..target].rfind('\n').map_or(target, |newline| newline + 1)
            };
            while !self.document.text.is_char_boundary(self.page_end) { self.page_end -= 1; }
        }
        let base = if paged { self.document.text[..self.page_start].chars().count() } else { 0 };
        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(primary.saturating_sub(base)),
            egui::text::CCursor::new(secondary.saturating_sub(base)),
        )));
        state.store(context, editor_id);
        self.page_cursor = None;
        true
    }

    pub fn refresh_after_workspace_edit(&mut self) {
        self.page_start = 0;
        self.page_end = 0;
        self.page_cursor = None;
        self.active_snippet_tab_stop = None;
        self.snippet_tab_stops.clear();
        self.pending_character_index = Some(self.document.character_index());
        self.highlighter = Highlighter::default();
    }

    pub fn select_char_range(&mut self, start: usize, end: usize) {
        self.active_snippet_tab_stop = None;
        self.snippet_tab_stops.clear();
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
        indent_size: usize,
        insert_spaces: bool,
    ) -> Option<u32> {
        if self.document.text.len() > PAGED_EDITOR_BYTES {
            self.active_snippet_tab_stop = None;
            self.snippet_tab_stops.clear();
            self.pending_selection = None;
            return self.render_paged_editor(
                ui,
                allow_edits && !self.close_after_save,
                breakpoints,
                current_line,
                diagnostics,
                indent_size,
                insert_spaces,
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
        let snippet_length_before_edit = self.active_snippet_tab_stop
            .map(|_| self.document.text.chars().count());
        let auto_deleted_pair = java && allow_edits && !self.close_after_save
            && auto_delete_java_pair(ui, &mut self.document.text, editor_id);
        if auto_deleted_pair {
            self.document.mark_changed();
        }
        let pair_edit = if java && allow_edits && !self.close_after_save && !auto_deleted_pair {
            auto_pair_java_character(ui, &mut self.document.text, editor_id)
        } else {
            JavaPairEdit::None
        };
        if pair_edit == JavaPairEdit::InsertedPair {
            self.document.mark_changed();
        }
        let auto_indented = java
            && allow_edits
            && !self.close_after_save
            && auto_indent_java_enter(ui, &mut self.document.text, editor_id, indent_size, insert_spaces);
        if auto_indented {
            self.document.mark_changed();
        }
        let highlighter = &mut self.highlighter;
        let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, _width: f32| {
            let mut job = highlighter.layout(ui, buffer.as_str(), language);
            job.wrap.max_width = f32::INFINITY;
            ui.fonts_mut(|fonts| fonts.layout_job(job))
        };
        let mut snippet_changed = auto_deleted_pair
            || pair_edit == JavaPairEdit::InsertedPair
            || auto_indented;
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
                    snippet_changed = true;
                    if !auto_indented && !auto_deleted_pair && pair_edit != JavaPairEdit::InsertedPair {
                        self.document.mark_changed();
                    }
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
        if snippet_changed && let Some(previous_length) = snippet_length_before_edit {
            self.adjust_snippet_tab_stops(previous_length);
            if self.sync_active_snippet_mirrors() {
                ui.ctx().request_repaint();
            }
        }
        if let Some(active) = self.active_snippet_tab_stop
            && let Some((_, ranges)) = self.snippet_tab_stops.get(active)
            && let Some((start, end)) = ranges.first()
            && !(*start..=*end).contains(&self.document.character_index())
        {
            self.active_snippet_tab_stop = None;
            self.snippet_tab_stops.clear();
        }
        toggled
    }

    fn adjust_snippet_tab_stops(&mut self, previous_length: usize) {
        let Some(active) = self.active_snippet_tab_stop else { return; };
        let Some((_, active_ranges)) = self.snippet_tab_stops.get(active) else {
            self.active_snippet_tab_stop = None;
            self.snippet_tab_stops.clear();
            return;
        };
        let Some((_, active_end)) = active_ranges.first().copied() else { return; };
        let current_length = self.document.text.chars().count();
        let delta = current_length as isize - previous_length as isize;
        if delta == 0 { return; }
        for (group_index, (_, ranges)) in self.snippet_tab_stops.iter_mut().enumerate() {
            for (range_index, (start, end)) in ranges.iter_mut().enumerate() {
                if group_index == active && range_index == 0 {
                    *end = (*end as isize + delta).max(*start as isize) as usize;
                } else if *start >= active_end {
                    *start = (*start as isize + delta).max(0) as usize;
                    *end = (*end as isize + delta).max(0) as usize;
                }
            }
        }
    }

    fn sync_active_snippet_mirrors(&mut self) -> bool {
        let Some(active_group) = self.active_snippet_tab_stop else { return false; };
        let Some((active_index, active_ranges)) = self.snippet_tab_stops.get(active_group) else { return false; };
        if *active_index == 0 { return false; }
        let Some((active_start, active_end)) = active_ranges.first().copied() else { return false; };
        let replacement: String = self.document.text.chars()
            .skip(active_start)
            .take(active_end.saturating_sub(active_start))
            .collect();
        let mut mirrors: Vec<_> = active_ranges.iter().copied().enumerate().skip(1).collect();
        mirrors.sort_by_key(|(_, (start, _))| std::cmp::Reverse(*start));
        let mut cursor_shift = 0isize;
        let mut changed = false;
        for (range_index, (start, end)) in mirrors {
            let existing: String = self.document.text.chars().skip(start).take(end.saturating_sub(start)).collect();
            if existing == replacement { continue; }
            let start_byte = byte_index(&self.document.text, start);
            let end_byte = byte_index(&self.document.text, end);
            self.document.text.replace_range(start_byte..end_byte, &replacement);
            let delta = replacement.chars().count() as isize - end.saturating_sub(start) as isize;
            if start < self.document.character_index() { cursor_shift += delta; }
            for (group_index, (_, ranges)) in self.snippet_tab_stops.iter_mut().enumerate() {
                for (candidate_index, (candidate_start, candidate_end)) in ranges.iter_mut().enumerate() {
                    if group_index == active_group && candidate_index == range_index {
                        *candidate_start = start;
                        *candidate_end = start + replacement.chars().count();
                    } else if *candidate_start >= end {
                        *candidate_start = (*candidate_start as isize + delta).max(0) as usize;
                        *candidate_end = (*candidate_end as isize + delta).max(0) as usize;
                    } else if *candidate_start < end && *candidate_end > start {
                        let mapped_end = (*candidate_end as isize + delta).max(*candidate_start as isize) as usize;
                        *candidate_end = mapped_end;
                    }
                }
            }
            changed = true;
        }
        if cursor_shift != 0 {
            let cursor = (self.document.character_index() as isize + cursor_shift).max(0) as usize;
            self.document.set_cursor(cursor);
        }
        if changed { self.document.mark_changed(); }
        changed
    }

    /// Edit a bounded page while retaining the complete document in Document::text.
    fn render_paged_editor(
        &mut self,
        ui: &mut egui::Ui,
        allow_edits: bool,
        breakpoints: &[u32],
        current_line: Option<u32>,
        diagnostics: &[crate::java::lsp::LanguageDiagnostic],
        indent_size: usize,
        insert_spaces: bool,
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
        let auto_deleted_pair = super::highlighter::SourceLanguage::for_path(&self.path)
            == super::highlighter::SourceLanguage::Java && allow_edits
            && auto_delete_java_pair(ui, &mut page, editor_id);
        let pair_edit = if !auto_deleted_pair && super::highlighter::SourceLanguage::for_path(&self.path)
            == super::highlighter::SourceLanguage::Java && allow_edits
        {
            auto_pair_java_character(ui, &mut page, editor_id)
        } else {
            JavaPairEdit::None
        };
        let auto_indented = super::highlighter::SourceLanguage::for_path(&self.path)
            == super::highlighter::SourceLanguage::Java
            && allow_edits
            && auto_indent_java_enter(ui, &mut page, editor_id, indent_size, insert_spaces);
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
        if output.response.changed() || auto_indented || auto_deleted_pair || pair_edit == JavaPairEdit::InsertedPair {
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
    let mut editor_command = None;
    let mut breakpoint_action = None;
    let indent_size = state.settings.java_indent_size.clamp(1, 8);
    let insert_spaces = state.settings.java_insert_spaces;
    if let Some(tab) = state
        .active_tab
        .and_then(|index| state.editor_tabs.get_mut(index))
    {
        ui.label(tab.path.display().to_string());
        ui.separator();
        let lines = state.debugger.breakpoints.lines(&tab.path);
        let current_line = state.debugger.inspector.selected_frame.and_then(|id| state.debugger.inspector.frames.iter().find(|frame| frame.id == id))
            .filter(|frame| frame.source.as_ref() == Some(&tab.path)).map(|frame| frame.line);
        let revision = tab.document.revision;
        let toggled = tab.render_editor(ui, !state.quit_approved, &lines, current_line, diagnostics,
            indent_size, insert_spaces);
        let path = tab.path.clone();
        if tab.document.revision != revision
            && path.extension().is_some_and(|extension| extension == "java")
            && state.language.can_request_for(&path)
        {
            let command = ui.input(|input| {
                java_completion_trigger(&tab.document, &input.events).or_else(|| {
                    input.events.iter().rev().find_map(|event| match event {
                        egui::Event::Text(text) | egui::Event::Paste(text) if text == "(" || text == "," => {
                            Some(crate::java::lsp::EditorCommand::SignatureHelp)
                        }
                        _ => None,
                    })
                })
            });
            editor_command = command;
        }
        let modified = tab.document.is_dirty();
        if modified { state.debugger.breakpoints.modified.insert(path.clone()); }
        else { state.debugger.breakpoints.modified.remove(&path); }
        if let Some(line) = toggled {
            breakpoint_action = Some(crate::app::events::DebuggerAction::ToggleBreakpoint {
                source: path,
                line,
                modified,
            });
        }
    }
    if let Some(command) = editor_command {
        state.enqueue(AppEvent::EditorCommand(command));
    }
    if let Some(action) = breakpoint_action {
        state.enqueue(AppEvent::Debugger(action));
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
