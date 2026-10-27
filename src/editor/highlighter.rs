use std::path::Path;
use std::sync::OnceLock;

use eframe::egui::{self, text::LayoutJob};
use super::document::PAGED_EDITOR_BYTES;
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
            job.append(text, 0.0, egui::TextFormat {
                font_id: egui::FontId::monospace(14.0),
                color: ui.visuals().text_color(),
                ..Default::default()
            });
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
