use std::path::PathBuf;

use eframe::egui;

use super::events::AppEvent;
use super::quick_open::QuickOpen;
use super::symbol_search::SymbolSearch;
use crate::java::lsp::{LanguageTool, Location};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchTab {
    #[default]
    Files,
    JavaSymbols,
}

#[derive(Default)]
pub struct SearchEverywhere {
    visible: bool,
    active_tab: SearchTab,
    query: String,
    focus_query: bool,
}

impl SearchEverywhere {
    pub fn open(&mut self, tab: SearchTab) {
        self.visible = true;
        self.active_tab = tab;
        self.query.clear();
        self.focus_query = true;
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.focus_query = false;
    }

    pub fn render(
        &mut self,
        context: &egui::Context,
        files: &mut QuickOpen,
        symbols: &mut SymbolSearch,
        language: &LanguageTool,
    ) -> (Option<PathBuf>, Vec<AppEvent>, Option<Location>) {
        let mut opened_file = None;
        let mut actions = Vec::new();
        let mut location = None;
        if !self.visible {
            return (opened_file, actions, location);
        }
        let mut open = self.visible;
        egui::Window::new(crate::i18n::text(context, "Search Everywhere"))
            .collapsible(false)
            .resizable(false)
            .fixed_size([600.0, 440.0])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(context, |ui| {
                (opened_file, actions, location) =
                    self.render_contents(ui, files, symbols, language);
            });
        let close = context.input(|input| input.key_pressed(egui::Key::Escape));
        self.visible = open && !close && opened_file.is_none() && location.is_none();
        (opened_file, actions, location)
    }

    fn render_contents(
        &mut self,
        ui: &mut egui::Ui,
        files: &mut QuickOpen,
        symbols: &mut SymbolSearch,
        language: &LanguageTool,
    ) -> (Option<PathBuf>, Vec<AppEvent>, Option<Location>) {
        ui.horizontal(|ui| {
            if ui
                .selectable_label(
                    self.active_tab == SearchTab::Files,
                    crate::i18n::ui_text(ui, "Files"),
                )
                .clicked()
            {
                self.active_tab = SearchTab::Files;
            }
            if ui
                .selectable_label(
                    self.active_tab == SearchTab::JavaSymbols,
                    crate::i18n::ui_text(ui, "Java Symbols"),
                )
                .clicked()
            {
                self.active_tab = SearchTab::JavaSymbols;
            }
        });
        let hint = match self.active_tab {
            SearchTab::Files => "Type a file name or path…",
            SearchTab::JavaSymbols => "Class, method, or field name…",
        };
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .hint_text(crate::i18n::ui_text(ui, hint))
                .desired_width(f32::INFINITY),
        );
        if self.focus_query {
            response.request_focus();
            self.focus_query = false;
        }
        match self.active_tab {
            SearchTab::Files => (files.render_results(ui, &mut self.query), Vec::new(), None),
            SearchTab::JavaSymbols => {
                let (actions, location) = symbols.render_results(ui, &self.query, language);
                (None, actions, location)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{quick_open::QuickOpen, symbol_search::SymbolSearch};
    use crate::i18n::{LanguagePreference, Translator};
    use crate::java::lsp::LanguageTool;
    use eframe::egui;

    type ResultSet = (
        Option<std::path::PathBuf>,
        Vec<crate::app::events::AppEvent>,
        Option<crate::java::lsp::Location>,
    );

    fn frame(
        context: &egui::Context,
        search: &mut SearchEverywhere,
        files: &mut QuickOpen,
        symbols: &mut SymbolSearch,
        language: &LanguageTool,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, ResultSet) {
        context.global_style_mut(|style| style.animation_time = 0.0);
        let mut result = (None, Vec::new(), None);
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                events,
                ..Default::default()
            },
            |root| {
                egui::CentralPanel::default().show(root, |ui| {
                    ui.heading(crate::i18n::ui_text(ui, "Search Everywhere"));
                    result = search.render_contents(ui, files, symbols, language);
                });
            },
        );
        output.textures_delta.clear();
        (output, result)
    }

    fn positions(shape: &egui::epaint::Shape, label: &str, result: &mut Vec<egui::Pos2>) {
        match shape {
            egui::epaint::Shape::Text(text) if text.galley.text() == label => {
                result.push(text.pos + text.galley.size() / 2.0);
            }
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    positions(shape, label, result);
                }
            }
            _ => {}
        }
    }

    fn click(position: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(position),
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    #[test]
    fn opening_each_entry_point_selects_its_tab_and_clears_query() {
        let mut search = SearchEverywhere::default();
        search.query = "Customer".into();
        search.open(SearchTab::JavaSymbols);
        assert_eq!(search.active_tab, SearchTab::JavaSymbols);
        assert!(search.query.is_empty());

        search.query = "Main".into();
        search.open(SearchTab::Files);
        assert_eq!(search.active_tab, SearchTab::Files);
        assert!(search.query.is_empty());
    }

    #[test]
    fn switching_tabs_keeps_query_and_requests_symbols() {
        let context = egui::Context::default();
        let (translator, _) = Translator::load(LanguagePreference::English, None);
        crate::i18n::install(&context, &translator);
        let mut search = SearchEverywhere::default();
        search.open(SearchTab::Files);
        search.query = "Customer".into();
        let mut files = QuickOpen::default();
        let mut symbols = SymbolSearch::default();
        let language = LanguageTool::default();

        let (output, _) = frame(
            &context,
            &mut search,
            &mut files,
            &mut symbols,
            &language,
            Vec::new(),
        );
        let mut points = Vec::new();
        for shape in &output.shapes {
            positions(&shape.shape, "Java Symbols", &mut points);
        }
        frame(
            &context,
            &mut search,
            &mut files,
            &mut symbols,
            &language,
            click(*points.last().expect("Java Symbols tab")),
        );

        assert_eq!(search.active_tab, SearchTab::JavaSymbols);
        assert_eq!(search.query, "Customer");
        assert!(matches!(
            frame(
                &context,
                &mut search,
                &mut files,
                &mut symbols,
                &language,
                Vec::new(),
            )
            .1
            .1
            .as_slice(),
            [crate::app::events::AppEvent::SearchWorkspaceSymbols(query)] if query == "Customer"
        ));
    }

    #[test]
    fn labels_are_localized_and_empty_enter_selects_nothing() {
        for (preference, labels) in [
            (
                LanguagePreference::English,
                ["Search Everywhere", "Files", "Java Symbols"],
            ),
            (
                LanguagePreference::Italian,
                ["Cerca ovunque", "File", "Simboli Java"],
            ),
        ] {
            let context = egui::Context::default();
            let (translator, _) = Translator::load(preference, None);
            crate::i18n::install(&context, &translator);
            let mut search = SearchEverywhere::default();
            search.open(SearchTab::Files);
            let mut files = QuickOpen::default();
            let mut symbols = SymbolSearch::default();
            let language = LanguageTool::default();
            let (output, result) = frame(
                &context,
                &mut search,
                &mut files,
                &mut symbols,
                &language,
                vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            assert!(result.0.is_none() && result.2.is_none());
            let rendered = format!("{:?}", output.shapes);
            for label in labels {
                assert!(rendered.contains(label), "missing label {label}");
            }
            if preference == LanguagePreference::English {
                assert!(rendered.contains("Open a project to search files"));
            }
        }
    }
}
