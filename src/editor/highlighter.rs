use std::path::Path;
use std::sync::OnceLock;

use super::document::PAGED_EDITOR_BYTES;
use eframe::egui::{self, text::LayoutJob};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

pub const MAX_HIGHLIGHT_BYTES: usize = PAGED_EDITOR_BYTES;

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
    let target = text
        .chars()
        .nth(cursor)
        .filter(|character| is_delimiter(*character))
        .map(|_| cursor)
        .or_else(|| {
            cursor
                .checked_sub(1)
                .filter(|index| text.chars().nth(*index).is_some_and(is_delimiter))
        });
    let Some(target) = target else {
        return;
    };
    let characters: Vec<_> = text.chars().collect();
    let Some((open, close)) = matching_java_delimiters(&characters, target) else {
        return;
    };
    let painter = ui.painter().with_clip_rect(output.text_clip_rect);
    for index in [open, close] {
        let start = output
            .galley
            .pos_from_cursor(egui::text::CCursor::new(index))
            .translate(output.galley_pos.to_vec2());
        let end = output
            .galley
            .pos_from_cursor(egui::text::CCursor::new(index + 1))
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

fn matching_java_delimiters(characters: &[char], target: usize) -> Option<(usize, usize)> {
    #[derive(Clone, Copy)]
    enum State {
        Code,
        LineComment,
        BlockComment,
        String,
        Character,
        TextBlock,
    }

    fn matches(open: char, close: char) -> bool {
        matches!((open, close), ('(', ')') | ('{', '}') | ('[', ']'))
    }

    let target_character = *characters.get(target)?;
    let wanted_open = matches!(target_character, '(' | '{' | '[');
    let mut state = State::Code;
    let mut stack = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        match state {
            State::LineComment => {
                if character == '\n' {
                    state = State::Code;
                }
            }
            State::BlockComment => {
                if character == '*' && characters.get(index + 1) == Some(&'/') {
                    index += 1;
                    state = State::Code;
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
                    index += 1;
                    state = State::BlockComment;
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
                ')' | '}' | ']' => {
                    if index == target && !wanted_open {
                        return stack
                            .last()
                            .filter(|(open, _)| matches(*open, character))
                            .map(|(_, open_index)| (*open_index, index));
                    }
                    if let Some((open, open_index)) = stack.last().copied()
                        && matches(open, character)
                    {
                        stack.pop();
                        if open_index == target && wanted_open {
                            debug_assert!(matches(characters[open_index], character));
                            return Some((open_index, index));
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
        static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
        static THEMES: OnceLock<ThemeSet> = OnceLock::new();
        let syntaxes = SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines);
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
