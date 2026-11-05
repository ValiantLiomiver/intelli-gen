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
    JavaClasses,
    Actions,
}

#[derive(Default)]
pub struct SearchEverywhere {
    visible: bool,
    active_tab: SearchTab,
    class_only: bool,
    query: String,
    focus_query: bool,
    action_query: String,
    selected_action: usize,
}

impl SearchEverywhere {
    pub fn open(&mut self, tab: SearchTab) {
        self.visible = true;
        self.selected_action = 0;
        self.action_query.clear();
        self.class_only = tab == SearchTab::JavaClasses;
        self.active_tab = if self.class_only {
            SearchTab::JavaSymbols
        } else {
            tab
        };
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
        recent_files: &[PathBuf],
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
                    self.render_contents(ui, files, symbols, language, recent_files);
            });
        let close = context.input(|input| input.key_pressed(egui::Key::Escape));
        let selected_action = self.active_tab == SearchTab::Actions && !actions.is_empty();
        self.visible =
            open && !close && opened_file.is_none() && location.is_none() && !selected_action;
        (opened_file, actions, location)
    }

    fn render_contents(
        &mut self,
        ui: &mut egui::Ui,
        files: &mut QuickOpen,
        symbols: &mut SymbolSearch,
        language: &LanguageTool,
        recent_files: &[PathBuf],
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
                self.class_only = false;
            }
            if ui
                .selectable_label(
                    self.active_tab == SearchTab::JavaSymbols,
                    crate::i18n::ui_text(ui, "Java Symbols"),
                )
                .clicked()
            {
                self.active_tab = SearchTab::JavaSymbols;
                self.class_only = false;
            }
            if ui
                .selectable_label(
                    self.active_tab == SearchTab::Actions,
                    crate::i18n::ui_text(ui, "Actions"),
                )
                .clicked()
            {
                self.active_tab = SearchTab::Actions;
                self.class_only = false;
            }
        });
        let hint = match self.active_tab {
            SearchTab::Files => "Type a file name or path…",
            SearchTab::JavaSymbols | SearchTab::JavaClasses if self.class_only => "Class name…",
            SearchTab::JavaSymbols | SearchTab::JavaClasses => "Class, method, or field name…",
            SearchTab::Actions => "Action name…",
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
            SearchTab::Files => (
                files.render_results(ui, &mut self.query, recent_files),
                Vec::new(),
                None,
            ),
            SearchTab::JavaSymbols | SearchTab::JavaClasses => {
                let (actions, location) =
                    symbols.render_results(ui, &self.query, language, self.class_only);
                (None, actions, location)
            }
            SearchTab::Actions => (
                None,
                self.render_action_results(ui).into_iter().collect(),
                None,
            ),
        }
    }

    fn render_action_results(&mut self, ui: &mut egui::Ui) -> Option<AppEvent> {
        if self.action_query != self.query {
            self.action_query.clone_from(&self.query);
            self.selected_action = 0;
        }
        let query = self.query.trim().to_lowercase().replace(' ', "_");
        let items = [
            (
                "Go to File…    ⇧⌘O",
                AppEvent::SearchEverywhereOpen(SearchTab::Files),
            ),
            (
                "Recent Files…    ⌘E",
                AppEvent::SearchEverywhereOpen(SearchTab::Files),
            ),
            (
                "Go to Class…    ⌘O",
                AppEvent::SearchEverywhereOpen(SearchTab::JavaClasses),
            ),
            (
                "Go to Java Symbol…    ⌘⌥O",
                AppEvent::SearchEverywhereOpen(SearchTab::JavaSymbols),
            ),
            (
                "Search Everywhere…    ⇧⇧",
                AppEvent::SearchEverywhereOpen(SearchTab::Files),
            ),
            (
                "Find Action…    ⌘⇧A",
                AppEvent::SearchEverywhereOpen(SearchTab::Actions),
            ),
            ("Go to Line…    ⌘L", AppEvent::GoToLineOpen),
            ("Show Project sidebar    ⌘1", AppEvent::ShowProjectPanel),
            (
                "Next Problem    F2",
                AppEvent::NavigateProblem { next: true },
            ),
            (
                "Previous Problem    ⇧F2",
                AppEvent::NavigateProblem { next: false },
            ),
            ("File Structure…    ⌘F12", AppEvent::FileStructureOpen),
            (
                "Evaluate Expression…    ⌥F8",
                AppEvent::EvaluateExpressionOpen,
            ),
            (
                "Problems    ⌘6",
                AppEvent::ShowPanel(super::ActivePanel::Problems),
            ),
            (
                "Test / production source    ⌘⇧T",
                AppEvent::ToggleTestSource,
            ),
            (
                "Toggle line breakpoint    ⌘F8",
                AppEvent::ToggleBreakpointAtCaret,
            ),
            ("View Breakpoints…    ⌘⇧F8", AppEvent::ShowBreakpoints),
            (
                "Toggle line bookmark    F3",
                AppEvent::ToggleBookmarkAtCaret,
            ),
            ("Show Bookmarks    ⌘F3", AppEvent::ShowBookmarks),
            (
                "Last Edit Location    ⇧⌘⌫",
                AppEvent::NavigateLastEditLocation,
            ),
            (
                "Find in File…    ⌘F",
                AppEvent::Find(crate::editor::find_replace::FindAction::Open { replace: false }),
            ),
            (
                "Replace in File…    ⌘R",
                AppEvent::Find(crate::editor::find_replace::FindAction::Open { replace: true }),
            ),
            (
                "Find in Project…    ⇧⌘F",
                AppEvent::Search(super::events::SearchAction::Open),
            ),
            (
                "Go to declaration    ⌘B",
                AppEvent::EditorCommand(crate::java::lsp::EditorCommand::Declaration),
            ),
            (
                "Go to definition",
                AppEvent::EditorCommand(crate::java::lsp::EditorCommand::Definition),
            ),
            (
                "Go to type definition    ⇧⌘B",
                AppEvent::EditorCommand(crate::java::lsp::EditorCommand::TypeDefinition),
            ),
            (
                "Go to implementation    ⌥⌘B",
                AppEvent::EditorCommand(crate::java::lsp::EditorCommand::Implementation),
            ),
            (
                "Find usages    ⌥F7",
                AppEvent::EditorCommand(crate::java::lsp::EditorCommand::References),
            ),
            (
                "Quick fixes    ⌥↩",
                AppEvent::EditorCommand(crate::java::lsp::EditorCommand::CodeActions),
            ),
            ("Rename Symbol…    Shift+F6", AppEvent::RenameSymbolOpen),
            (
                "Organize imports    ⌃⌥O",
                AppEvent::EditorCommand(crate::java::lsp::EditorCommand::OrganizeImports),
            ),
            (
                "Reformat code    ⌥⌘L",
                AppEvent::EditorCommand(crate::java::lsp::EditorCommand::Format),
            ),
            ("Save                        ⌘S", AppEvent::SaveActiveFile),
            ("Save All                 ⇧⌘S", AppEvent::SaveAllFiles),
            ("Close File                ⌘W", AppEvent::CloseActiveFile),
            ("Build", AppEvent::Build(super::events::BuildAction::Build)),
            ("Test", AppEvent::Build(super::events::BuildAction::Test)),
            ("Clean", AppEvent::Build(super::events::BuildAction::Clean)),
            ("Run Context Configuration    ⌃R", AppEvent::RunContext),
            (
                "Debug Selected Configuration    ⌃D",
                AppEvent::Debugger(super::events::DebuggerAction::Start),
            ),
            (
                "Terminal    ⌥F12",
                AppEvent::Terminal(super::events::TerminalAction::Open),
            ),
        ];
        let mut results: Vec<_> = items
            .iter()
            .filter_map(|(label, event)| {
                let score = action_search_score(ui, label, &query)?;
                Some((label, event, score))
            })
            .collect();
        if !query.is_empty() {
            results.sort_by(|left, right| right.2.cmp(&left.2));
        }
        let (up, down, enter) = ui.input(|input| {
            (
                input.key_pressed(egui::Key::ArrowUp),
                input.key_pressed(egui::Key::ArrowDown),
                input.key_pressed(egui::Key::Enter),
            )
        });
        if up && !results.is_empty() {
            self.selected_action = self.selected_action.saturating_sub(1);
        }
        if down && !results.is_empty() {
            self.selected_action = (self.selected_action + 1).min(results.len() - 1);
        }
        let mut chosen = enter
            .then(|| results.get(self.selected_action).map(|item| item.1.clone()))
            .flatten();
        if results.is_empty() {
            ui.weak(crate::i18n::ui_text(ui, "No matching actions"));
        }
        egui::ScrollArea::vertical()
            .max_height(310.0)
            .show(ui, |ui| {
                for (index, (label, event, _)) in results.iter().enumerate() {
                    let response = ui.selectable_label(
                        index == self.selected_action,
                        crate::i18n::ui_text(ui, label),
                    );
                    if response.clicked() {
                        chosen = Some((**event).clone());
                    }
                    if response.hovered() {
                        self.selected_action = index;
                    }
                }
            });
        chosen
    }
}

fn action_search_score(ui: &egui::Ui, label: &str, query: &str) -> Option<i64> {
    let translated = crate::i18n::ui_text(ui, label)
        .to_lowercase()
        .replace(' ', "_");
    let english = label.to_lowercase().replace(' ', "_");
    let aliases = action_search_aliases(label);
    let mut names = vec![translated.as_str(), english.as_str()];
    names.extend(aliases.iter().copied());
    let score = names
        .iter()
        .filter_map(|name| crate::app::quick_open::fuzzy_score(name, name, query))
        .max();
    score.or_else(|| action_typo_score(&names, query))
}

fn action_search_aliases(label: &str) -> &'static [&'static str] {
    match label {
        "Go to declaration    ⌘B" => &["go_to_declaration", "vai_alla_dichiarazione"],
        "Test / production source    ⌘⇧T" => &["test_subject", "vai_al_test"],
        "Toggle line breakpoint    ⌘F8" => &["toggle_breakpoint", "punto_interruzione"],
        "View Breakpoints…    ⌘⇧F8" => &["view_breakpoints", "visualizza_breakpoint"],
        "Next Problem    F2" => &["next_problem", "problema_successivo"],
        "Previous Problem    ⇧F2" => &["previous_problem", "problema_precedente"],
        "Problems    ⌘6" => &["problems", "problemi", "error_list", "elenco_errori"],
        "Evaluate Expression…    ⌥F8" => &["evaluate_expression", "valuta_espressione"],
        "Last Edit Location    ⇧⌘⌫" => &["last_edit_location", "ultima_modifica"],
        "Toggle line bookmark    F3" => {
            &["toggle_bookmark", "aggiungi_segnalibro", "segnalibro_riga"]
        }
        "Show Bookmarks    ⌘F3" => &["show_bookmarks", "mostra_segnalibri"],
        "Show Project sidebar    ⌘1" => {
            &["show_project_window", "project_sidebar", "mostra_progetto"]
        }
        "Find usages    ⌥F7" => &["references", "riferimenti"],
        "Quick fixes    ⌥↩" => &["intentions", "correzioni_rapide"],
        "Organize imports    ⌃⌥O" => &["optimize_imports", "ottimizza_importazioni"],
        "Reformat code    ⌥⌘L" => &["format_code", "formatta_codice"],
        "Build" => &["compile_project", "compila_progetto"],
        "Test" => &["run_tests", "esegui_test"],
        "Run Context Configuration    ⌃R" => {
            &["run_context", "run_current_test", "esegui_test_corrente"]
        }
        "Save All                 ⇧⌘S" => &["save_all_files", "salva_tutti_i_file"],
        "Terminal" => &["command_line", "riga_di_comando"],
        _ => &[],
    }
}

fn action_typo_score(names: &[&str], query: &str) -> Option<i64> {
    let query_words: Vec<_> = query.split('_').filter(|word| !word.is_empty()).collect();
    if query_words.is_empty() {
        return None;
    }
    let words: Vec<_> = names
        .iter()
        .flat_map(|name| name.split(|character: char| !character.is_alphanumeric()))
        .filter(|word| !word.is_empty())
        .collect();
    let mut typo_used = false;
    let mut score = -100;
    for query_word in query_words {
        if words.iter().any(|word| word.contains(query_word)) {
            score += 10;
        } else if !typo_used
            && query_word.chars().count() >= 4
            && words.iter().any(|word| one_edit_apart(word, query_word))
        {
            typo_used = true;
            score += 1;
        } else {
            return None;
        }
    }
    Some(score)
}

fn one_edit_apart(left: &str, right: &str) -> bool {
    let left: Vec<_> = left.chars().collect();
    let right: Vec<_> = right.chars().collect();
    if left.len().abs_diff(right.len()) > 1 {
        return false;
    }
    let (mut left_index, mut right_index, mut edits) = (0, 0, 0);
    while left_index < left.len() && right_index < right.len() {
        if left[left_index] == right[right_index] {
            left_index += 1;
            right_index += 1;
        } else {
            edits += 1;
            if edits > 1 {
                return false;
            }
            if left.len() == right.len()
                && left_index + 1 < left.len()
                && left[left_index] == right[right_index + 1]
                && left[left_index + 1] == right[right_index]
            {
                return left[left_index + 2..] == right[right_index + 2..];
            }
            match left.len().cmp(&right.len()) {
                std::cmp::Ordering::Greater => left_index += 1,
                std::cmp::Ordering::Less => right_index += 1,
                std::cmp::Ordering::Equal => {
                    left_index += 1;
                    right_index += 1;
                }
            }
        }
    }
    edits + usize::from(left_index < left.len() || right_index < right.len()) <= 1
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
                    result = search.render_contents(ui, files, symbols, language, &[]);
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
