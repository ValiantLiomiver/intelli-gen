use eframe::egui;

use super::events::AppEvent;
use crate::java::lsp::{LanguageTool, Location};

#[derive(Default)]
pub struct SymbolSearch {
    visible: bool,
    query: String,
    selected: usize,
    focus_query: bool,
}

impl SymbolSearch {
    pub fn open(&mut self) {
        self.visible = true;
        self.query.clear();
        self.selected = 0;
        self.focus_query = true;
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    pub fn render(
        &mut self,
        ctx: &egui::Context,
        language: &LanguageTool,
    ) -> (Vec<AppEvent>, Option<Location>) {
        let mut actions = Vec::new();
        let mut chosen = None;
        if !self.visible {
            return (actions, chosen);
        }
        let mut open = self.visible;
        let mut close = false;
        egui::Window::new("Go to Java Symbol")
            .collapsible(false)
            .resizable(false)
            .fixed_size([620.0, 440.0])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("Class, method, or field name…")
                        .desired_width(f32::INFINITY),
                );
                if self.focus_query {
                    response.request_focus();
                    self.focus_query = false;
                }
                if response.changed() {
                    self.selected = 0;
                    actions.push(AppEvent::SearchWorkspaceSymbols(self.query.clone()));
                } else if !self.query.is_empty() && language.workspace_symbol_query != self.query {
                    actions.push(AppEvent::SearchWorkspaceSymbols(self.query.clone()));
                }
                let (up, down, enter, escape) = ui.input(|input| {
                    (
                        input.key_pressed(egui::Key::ArrowUp),
                        input.key_pressed(egui::Key::ArrowDown),
                        input.key_pressed(egui::Key::Enter),
                        input.key_pressed(egui::Key::Escape),
                    )
                });
                if escape {
                    close = true;
                }
                let matching_query = language.workspace_symbol_query == self.query;
                let symbols = if matching_query {
                    &language.workspace_symbols[..]
                } else {
                    &[]
                };
                if up && !symbols.is_empty() {
                    self.selected = self.selected.saturating_sub(1);
                }
                if down && !symbols.is_empty() {
                    self.selected = (self.selected + 1).min(symbols.len() - 1);
                }
                if enter && let Some(symbol) = symbols.get(self.selected) {
                    chosen = Some(symbol.location.clone());
                    close = true;
                }
                if self.query.trim().is_empty() {
                    ui.weak("Type at least one character to search the Java workspace.");
                } else if language.workspace_symbols_loading {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Searching Java symbols…");
                    });
                } else if let Some(error) = &language.workspace_symbols_error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                } else if matching_query && symbols.is_empty() {
                    ui.weak("No matching symbols");
                }
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        for (index, symbol) in symbols.iter().enumerate() {
                            let detail = symbol.container.as_deref().unwrap_or("");
                            let file_name = symbol
                                .location
                                .path
                                .file_name()
                                .unwrap_or(symbol.location.path.as_os_str())
                                .to_string_lossy();
                            let label = format!(
                                "{}  {}  {}  {}:{}",
                                kind_name(symbol.kind),
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
                                close = true;
                            }
                            if response.hovered() {
                                self.selected = index;
                            }
                        }
                    });
            });
        self.visible = open && !close;
        (actions, chosen)
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
