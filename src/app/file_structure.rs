use eframe::egui;

use crate::java::lsp::{LanguageTool, Location, OutlineSymbol};

#[derive(Default)]
pub struct FileStructure {
    visible: bool,
    path: Option<std::path::PathBuf>,
    query: String,
    selected: usize,
    focus_query: bool,
}

impl FileStructure {
    pub fn open(&mut self, path: std::path::PathBuf) {
        self.visible = true;
        self.path = Some(path);
        self.query.clear();
        self.selected = 0;
        self.focus_query = true;
    }

    pub fn render(&mut self, ctx: &egui::Context, language: &LanguageTool) -> Option<Location> {
        if !self.visible {
            return None;
        }
        let mut chosen = None;
        let mut open = self.visible;
        let mut close = false;
        let symbols: &[OutlineSymbol] = if self.path == language.document_symbols_path {
            &language.document_symbols
        } else {
            &[]
        };
        let query = self.query.to_lowercase();
        let filtered: Vec<&OutlineSymbol> = symbols
            .iter()
            .filter(|symbol| query.is_empty() || symbol.name.to_lowercase().contains(&query))
            .collect();
        egui::Window::new("File Structure")
            .collapsible(false)
            .resizable(true)
            .fixed_size([560.0, 460.0])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("Filter classes, methods, and fields…")
                        .desired_width(f32::INFINITY),
                );
                if self.focus_query {
                    response.request_focus();
                    self.focus_query = false;
                }
                if response.changed() {
                    self.selected = 0;
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
                if up && !filtered.is_empty() {
                    self.selected = self.selected.saturating_sub(1);
                }
                if down && !filtered.is_empty() {
                    self.selected = (self.selected + 1).min(filtered.len() - 1);
                }
                if enter && let Some(symbol) = filtered.get(self.selected) {
                    chosen = Some(symbol.location.clone());
                    close = true;
                }
                if language.document_symbols_loading {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Loading Java file structure…");
                    });
                } else if let Some(error) = &language.document_symbols_error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                } else if symbols.is_empty() {
                    ui.weak("No symbols found in this Java file.");
                } else if filtered.is_empty() {
                    ui.weak("No matching symbols.");
                }
                egui::ScrollArea::vertical()
                    .max_height(380.0)
                    .show(ui, |ui| {
                        for (index, symbol) in filtered.iter().enumerate() {
                            let indent = "  ".repeat(symbol.depth.min(16));
                            let label = format!(
                                "{indent}{}  {}  :{}",
                                kind_name(symbol.kind),
                                symbol.name,
                                symbol.location.range.start.line + 1
                            );
                            let response = ui.selectable_label(index == self.selected, label);
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
        chosen
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
