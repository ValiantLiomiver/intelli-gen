use eframe::egui;

#[derive(Default)]
pub struct GoToLine {
    visible: bool,
    input: String,
    focus: bool,
}

impl GoToLine {
    pub fn open(&mut self, line: usize, column: usize) {
        self.visible = true;
        self.input = format!("{line}:{column}");
        self.focus = true;
    }

    pub fn render(&mut self, ctx: &egui::Context) -> Option<(usize, Option<usize>)> {
        if !self.visible {
            return None;
        }
        let mut open = self.visible;
        let mut chosen = None;
        let mut close = false;
        egui::Window::new(crate::i18n::text(ctx, "Go to Line:Column"))
            .collapsible(false)
            .resizable(false)
            .fixed_size([360.0, 105.0])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                let response = ui
                    .horizontal(|ui| {
                        ui.label(crate::i18n::ui_text(ui, "[Line] [:column]:"));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.input)
                                .desired_width(f32::INFINITY),
                        )
                    })
                    .inner;
                if self.focus {
                    response.request_focus();
                    let mut state = egui::text_edit::TextEditState::load(ui.ctx(), response.id)
                        .unwrap_or_default();
                    state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::two(
                            egui::text::CCursor::new(0),
                            egui::text::CCursor::new(self.input.chars().count()),
                        )));
                    state.store(ui.ctx(), response.id);
                    self.focus = false;
                }
                let (enter, escape) = ui.input(|input| {
                    (
                        input.key_pressed(egui::Key::Enter),
                        input.key_pressed(egui::Key::Escape),
                    )
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(crate::i18n::ui_text(ui, "OK")).clicked() || enter {
                        if let Some(position) = parse_position(&self.input) {
                            chosen = Some(position);
                            close = true;
                        }
                    }
                    if ui.button(crate::i18n::ui_text(ui, "Cancel")).clicked() || escape {
                        close = true;
                    }
                });
            });
        self.visible = open && !close;
        chosen
    }
}

fn parse_position(input: &str) -> Option<(usize, Option<usize>)> {
    let mut parts = input.trim().split(':');
    let line = parts.next()?.trim().parse::<usize>().ok()?;
    let column = parts.next().map(str::trim);
    if line == 0 || parts.next().is_some() {
        return None;
    }
    let column = match column {
        Some("") | None => None,
        Some(column) => {
            let column = column.parse::<usize>().ok()?;
            Some((column > 0).then_some(column)?)
        }
    };
    Some((line, column))
}
