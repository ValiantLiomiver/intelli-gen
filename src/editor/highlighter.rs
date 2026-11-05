use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use super::document::PAGED_EDITOR_BYTES;
use eframe::egui::{self, text::LayoutJob};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};
use syntect::util::LinesWithEndings;

pub const MAX_HIGHLIGHT_BYTES: usize = PAGED_EDITOR_BYTES;

fn syntax_set() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceLanguage {
    Plain,
    Java,
    Xml,
    Gradle,
    Yaml,
}

impl SourceLanguage {
    pub(super) fn for_path(path: &Path) -> Self {
        match path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("java") => Self::Java,
            Some("xml") => Self::Xml,
            Some("gradle") => Self::Gradle,
            Some("yaml" | "yml") => Self::Yaml,
            _ => Self::Plain,
        }
    }

    fn syntax_extension(self) -> Option<&'static str> {
        match self {
            Self::Java => Some("java"),
            Self::Xml => Some("xml"),
            Self::Gradle => Some("gradle"),
            Self::Yaml => Some("yaml"),
            Self::Plain => None,
        }
    }
}

pub(super) fn paint_java_delimiter_match(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    text: &str,
    cursor: usize,
) {
    let is_delimiter = |character: char| matches!(character, '(' | ')' | '{' | '}' | '[' | ']');
    let byte_cursor = text
        .char_indices()
        .nth(cursor)
        .map_or(text.len(), |(byte, _)| byte);
    let current = text[byte_cursor..].chars().next();
    let previous = text[..byte_cursor].chars().next_back();
    let angle_tokens = if matches!(current, Some('<' | '>')) || matches!(previous, Some('<' | '>'))
    {
        java_generic_angle_tokens(text)
    } else {
        HashSet::new()
    };
    let target = if angle_tokens.contains(&cursor) {
        Some(cursor)
    } else if current.is_some_and(is_delimiter) {
        Some(cursor)
    } else if previous == Some('@') && text[..byte_cursor].chars().rev().nth(1) == Some('{') {
        cursor.checked_sub(1)
    } else if previous.is_some_and(is_delimiter)
        || cursor
            .checked_sub(1)
            .is_some_and(|index| angle_tokens.contains(&index))
    {
        cursor.checked_sub(1)
    } else {
        None
    };
    let Some(target) = target else {
        return;
    };
    let characters: Vec<_> = text.chars().collect();
    let Some((open, close, open_length)) =
        matching_java_delimiters(&characters, target, &angle_tokens)
    else {
        return;
    };
    let painter = ui.painter().with_clip_rect(output.text_clip_rect);
    for (index, length) in [(open, open_length), (close, 1)] {
        let start = output
            .galley
            .pos_from_cursor(egui::text::CCursor::new(index))
            .translate(output.galley_pos.to_vec2());
        let end = output
            .galley
            .pos_from_cursor(egui::text::CCursor::new(index + length))
            .translate(output.galley_pos.to_vec2());
        let rect = egui::Rect::from_min_max(
            egui::pos2(start.left(), start.top()),
            egui::pos2(end.left().max(start.left() + 1.0), start.bottom()),
        );
        painter.rect_stroke(
            rect,
            1.0,
            egui::Stroke::new(1.0, ui.visuals().selection.stroke.color),
            egui::StrokeKind::Inside,
        );
    }
}

fn java_generic_angle_tokens(text: &str) -> HashSet<usize> {
    let syntaxes = syntax_set();
    let Some(syntax) = syntaxes.find_syntax_by_extension("java") else {
        return HashSet::new();
    };
    let mut parser = ParseState::new(syntax);
    let mut scopes = ScopeStack::default();
    let mut angle_tokens = HashSet::new();
    let mut char_offset = 0;
    for line in LinesWithEndings::from(text) {
        let Ok(operations) = parser.parse_line(line, syntaxes) else {
            return HashSet::new();
        };
        let mut operation = 0;
        let mut line_char_offset = 0;
        for (byte, character) in line.char_indices() {
            while operations.get(operation).is_some_and(|(at, _)| *at <= byte) {
                if scopes.apply(&operations[operation].1).is_err() {
                    return HashSet::new();
                }
                operation += 1;
            }
            let index = char_offset + line_char_offset;
            line_char_offset += 1;
            if matches!(character, '<' | '>')
                && scopes.scopes.last().is_some_and(|scope| {
                    let name = scope.build_string();
                    name.ends_with("punctuation.definition.generic.begin.java")
                        || name.ends_with("punctuation.definition.generic.end.java")
                })
            {
                angle_tokens.insert(index);
            }
        }
        while let Some((_, operation_to_apply)) = operations.get(operation) {
            if scopes.apply(operation_to_apply).is_err() {
                return HashSet::new();
            }
            operation += 1;
        }
        char_offset += line_char_offset;
    }
    angle_tokens
}

pub(super) fn is_java_code_at(text: &str, target: usize) -> bool {
    let syntaxes = syntax_set();
    let Some(syntax) = syntaxes.find_syntax_by_extension("java") else {
        return false;
    };
    let mut parser = ParseState::new(syntax);
    let mut scopes = ScopeStack::default();
    let mut char_offset = 0;
    for line in LinesWithEndings::from(text) {
        let Ok(operations) = parser.parse_line(line, syntaxes) else {
            return false;
        };
        let mut operation = 0;
        let mut line_char_offset = 0;
        for (byte, _) in line.char_indices() {
            while operations.get(operation).is_some_and(|(at, _)| *at <= byte) {
                if scopes.apply(&operations[operation].1).is_err() {
                    return false;
                }
                operation += 1;
            }
            if char_offset + line_char_offset == target {
                return scopes.scopes.iter().all(|scope| {
                    let name = scope.build_string();
                    !name.contains("comment")
                        && !name.contains("string")
                        && !name.contains("character")
                });
            }
            line_char_offset += 1;
        }
        while let Some((_, operation_to_apply)) = operations.get(operation) {
            if scopes.apply(operation_to_apply).is_err() {
                return false;
            }
            operation += 1;
        }
        char_offset += line_char_offset;
    }
    if char_offset == target {
        scopes.scopes.iter().all(|scope| {
            let name = scope.build_string();
            !name.contains("comment") && !name.contains("string") && !name.contains("character")
        })
    } else {
        false
    }
}

// shortcut: Java Unicode escapes are not preprocessed before lexing; add source mapping if they appear in real projects.
pub(super) fn is_matching_java_closing_brace(text: &str, target: usize) -> bool {
    let characters: Vec<_> = text.chars().take(target.saturating_add(1)).collect();
    characters.get(target) == Some(&'}')
        && matching_java_delimiters(&characters, target, &HashSet::new()).is_some()
}

pub(super) fn matching_java_open_brace_before(text: &str, target: usize) -> Option<usize> {
    let mut characters: Vec<_> = text.chars().take(target).collect();
    if target > characters.len() {
        return None;
    }
    characters.push('}');
    matching_java_delimiters(&characters, target, &HashSet::new()).map(|(open, _, _)| open)
}

fn matching_java_delimiters(
    characters: &[char],
    target: usize,
    angle_tokens: &HashSet<usize>,
) -> Option<(usize, usize, usize)> {
    #[derive(Clone, Copy)]
    enum State {
        Code,
        LineComment,
        BlockComment { javadoc: bool },
        String,
        Character,
        TextBlock,
    }

    fn matches(open: char, close: char) -> bool {
        matches!(
            (open, close),
            ('(', ')') | ('{', '}') | ('[', ']') | ('<', '>')
        )
    }

    let target_character = *characters.get(target)?;
    let wanted_open = matches!(target_character, '(' | '{' | '[' | '@')
        || (target_character == '<' && angle_tokens.contains(&target));
    let mut state = State::Code;
    let mut stack = Vec::new();
    let mut javadoc_stack = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        match state {
            State::LineComment => {
                if character == '\n' {
                    state = State::Code;
                }
            }
            State::BlockComment { javadoc } => {
                if character == '*' && characters.get(index + 1) == Some(&'/') {
                    index += 1;
                    state = State::Code;
                    javadoc_stack.clear();
                } else if javadoc && character == '{' && characters.get(index + 1) == Some(&'@') {
                    javadoc_stack.push(('{', index, true));
                } else if javadoc && matches!(character, '[' | '(') {
                    javadoc_stack.push((character, index, false));
                } else if javadoc {
                    if matches!(character, '}' | ']' | ')')
                        && let Some((open, open_index, inline_tag)) = javadoc_stack.last().copied()
                        && matches(open, character)
                    {
                        if index == target && !wanted_open {
                            return Some((open_index, index, if inline_tag { 2 } else { 1 }));
                        }
                        javadoc_stack.pop();
                        if wanted_open
                            && (open_index == target || (inline_tag && open_index + 1 == target))
                        {
                            return Some((open_index, index, if inline_tag { 2 } else { 1 }));
                        }
                    }
                }
            }
            State::String | State::Character => {
                if character == '\\' {
                    index += 1;
                } else if matches!(
                    (state, character),
                    (State::String, '"') | (State::Character, '\'')
                ) {
                    state = State::Code;
                }
            }
            State::TextBlock => {
                if character == '\\' {
                    index += 1;
                } else if character == '"'
                    && characters.get(index + 1) == Some(&'"')
                    && characters.get(index + 2) == Some(&'"')
                {
                    index += 2;
                    state = State::Code;
                }
            }
            State::Code => match character {
                '/' if characters.get(index + 1) == Some(&'/') => {
                    index += 1;
                    state = State::LineComment;
                }
                '/' if characters.get(index + 1) == Some(&'*') => {
                    let javadoc = characters.get(index + 2) == Some(&'*');
                    index += 1;
                    state = State::BlockComment { javadoc };
                }
                '"' if characters.get(index + 1) == Some(&'"')
                    && characters.get(index + 2) == Some(&'"') =>
                {
                    index += 2;
                    state = State::TextBlock;
                }
                '"' => state = State::String,
                '\'' => state = State::Character,
                '(' | '{' | '[' => stack.push((character, index)),
                '<' if angle_tokens.contains(&index) => stack.push((character, index)),
                ')' | '}' | ']' | '>' if character != '>' || angle_tokens.contains(&index) => {
                    if index == target && !wanted_open {
                        let open_index = stack
                            .last()
                            .filter(|(open, _)| matches(*open, character))
                            .map(|(_, open_index)| *open_index)?;
                        debug_assert!(matches(characters[open_index], character));
                        return Some((open_index, index, 1));
                    }
                    if let Some((open, open_index)) = stack.last().copied()
                        && matches(open, character)
                    {
                        stack.pop();
                        if open_index == target && wanted_open {
                            debug_assert!(matches(characters[open_index], character));
                            return Some((open_index, index, 1));
                        }
                    }
                }
                _ => {}
            },
        }
        index += 1;
    }
    None
}

#[derive(Default)]
pub struct Highlighter {
    cached: Option<(String, bool, SourceLanguage, LayoutJob)>,
}

impl Highlighter {
    pub(super) fn layout(
        &mut self,
        ui: &egui::Ui,
        text: &str,
        language: SourceLanguage,
    ) -> LayoutJob {
        let dark = ui.visuals().dark_mode;
        if let Some((cached_text, cached_dark, cached_language, job)) = &self.cached {
            if cached_text == text && *cached_dark == dark && *cached_language == language {
                return job.clone();
            }
        }
        // The editor pages files above this bound before they reach the parser.
        if language == SourceLanguage::Plain || text.len() > MAX_HIGHLIGHT_BYTES {
            let mut job = LayoutJob::default();
            job.append(
                text,
                0.0,
                egui::TextFormat {
                    font_id: egui::FontId::monospace(14.0),
                    color: ui.visuals().text_color(),
                    ..Default::default()
                },
            );
            self.cached = Some((text.to_owned(), dark, language, job.clone()));
            return job;
        }
        static THEMES: OnceLock<ThemeSet> = OnceLock::new();
        let syntaxes = syntax_set();
        let themes = THEMES.get_or_init(ThemeSet::load_defaults);
        let syntax = language
            .syntax_extension()
            .and_then(|extension| syntaxes.find_syntax_by_extension(extension))
            .unwrap_or_else(|| syntaxes.find_syntax_plain_text());
        let theme_name = if dark {
            "base16-ocean.dark"
        } else {
            "InspiredGitHub"
        };
        let mut job = LayoutJob::default();
        let font_id = egui::FontId::monospace(14.0);
        if let Some(theme) = themes.themes.get(theme_name) {
            let mut highlighter = HighlightLines::new(syntax, theme);
            for line in LinesWithEndings::from(text) {
                match highlighter.highlight_line(line, syntaxes) {
                    Ok(ranges) => {
                        for (style, section) in ranges {
                            job.append(
                                section,
                                0.0,
                                egui::TextFormat {
                                    font_id: font_id.clone(),
                                    color: egui::Color32::from_rgb(
                                        style.foreground.r,
                                        style.foreground.g,
                                        style.foreground.b,
                                    ),
                                    ..Default::default()
                                },
                            );
                        }
                    }
                    Err(_) => job.append(
                        line,
                        0.0,
                        egui::TextFormat {
                            font_id: font_id.clone(),
                            color: ui.visuals().text_color(),
                            ..Default::default()
                        },
                    ),
                }
            }
        } else {
            job.append(
                text,
                0.0,
                egui::TextFormat {
                    font_id,
                    color: ui.visuals().text_color(),
                    ..Default::default()
                },
            );
        }
        self.cached = Some((text.to_owned(), dark, language, job.clone()));
        job
    }
}
