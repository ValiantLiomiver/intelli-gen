use eframe::egui;

use super::events::AppEvent;
use crate::java::lsp::{LanguageTool, Location};

#[derive(Default)]
pub struct SymbolSearch {
    last_query: String,
    selected: usize,
}

impl SymbolSearch {
    pub fn reset_selection(&mut self) {
        self.selected = 0;
    }

    pub fn render_results(
        &mut self,
        ui: &mut egui::Ui,
        query: &str,
        language: &LanguageTool,
    ) -> (Vec<AppEvent>, Option<Location>) {
        let mut actions = Vec::new();
        let mut chosen = None;
        if self.last_query != query {
            self.last_query = query.to_owned();
            self.selected = 0;
        }
        if !query.is_empty() && language.workspace_symbol_query != query {
            actions.push(AppEvent::SearchWorkspaceSymbols(query.to_owned()));
        }
        let symbols = matching_symbols(query, language);
        let (up, down, enter) = ui.input(|input| {
            (
                input.key_pressed(egui::Key::ArrowUp),
                input.key_pressed(egui::Key::ArrowDown),
                input.key_pressed(egui::Key::Enter),
            )
        });
        if up && !symbols.is_empty() {
            self.selected = self.selected.saturating_sub(1);
        }
        if down && !symbols.is_empty() {
            self.selected = (self.selected + 1).min(symbols.len() - 1);
        }
        if enter && let Some(symbol) = symbols.get(self.selected) {
            chosen = Some(symbol.location.clone());
        }
        if query.trim().is_empty() {
            ui.weak(crate::i18n::ui_text(
                ui,
                "Type at least one character to search the Java workspace.",
            ));
        } else if language.workspace_symbols_loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(crate::i18n::ui_text(ui, "Searching Java symbols…"));
            });
        } else if let Some(error) = &language.workspace_symbols_error {
            ui.colored_label(
                ui.visuals().error_fg_color,
                crate::i18n::message(ui.ctx(), error),
            );
        } else if language.workspace_symbol_query == query && symbols.is_empty() {
            ui.weak(crate::i18n::ui_text(ui, "No matching symbols"));
        }
        egui::ScrollArea::vertical()
            .max_height(310.0)
            .show(ui, |ui| {
                for (index, symbol) in symbols.iter().enumerate() {
                    let kind = crate::i18n::ui_text(ui, kind_name(symbol.kind));
                    let detail = symbol.container.as_deref().unwrap_or("");
                    let file_name = symbol
                        .location
                        .path
                        .file_name()
                        .unwrap_or(symbol.location.path.as_os_str())
                        .to_string_lossy();
                    let label = format!(
                        "{}  {}  {}  {}:{}",
                        kind,
                        symbol.name,
                        detail,
                        file_name,
                        symbol.location.range.start.line + 1
                    );
                    let response = ui
                        .selectable_label(index == self.selected, label)
                        .on_hover_text(format!(
                            "{}:{}",
                            symbol.location.path.display(),
                            symbol.location.range.start.line + 1
                        ));
                    if response.clicked() {
                        chosen = Some(symbol.location.clone());
                    }
                    if response.hovered() {
                        self.selected = index;
                    }
                }
            });
        (actions, chosen)
    }
}

fn matching_symbols<'a>(
    query: &str,
    language: &'a LanguageTool,
) -> &'a [crate::java::lsp::WorkspaceSymbol] {
    if !query.is_empty() && language.workspace_symbol_query == query {
        &language.workspace_symbols
    } else {
        &[]
    }
}

fn kind_name(kind: u64) -> &'static str {
    match kind {
        5 => "class",
        6 => "method",
        7 => "property",
        8 => "field",
        9 => "constructor",
        10 => "enum",
        11 => "interface",
        12 => "function",
        13 => "variable",
        23 => "struct",
        26 => "type",
        _ => "symbol",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn ignores_workspace_symbols_for_an_older_query() {
        let mut language = LanguageTool::default();
        language.workspace_symbol_query = "OldName".into();
        language
            .workspace_symbols
            .push(crate::java::lsp::WorkspaceSymbol {
                name: "OldName".into(),
                kind: 5,
                container: None,
                location: Location {
                    path: PathBuf::from("/tmp/OldName.java"),
                    range: crate::java::lsp::Range {
                        start: crate::java::lsp::Position {
                            line: 0,
                            character: 0,
                        },
                        end: crate::java::lsp::Position {
                            line: 0,
                            character: 7,
                        },
                    },
                },
            });

        assert!(matching_symbols("NewName", &language).is_empty());
        assert_eq!(matching_symbols("OldName", &language).len(), 1);
    }
}
