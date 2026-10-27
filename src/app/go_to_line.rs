use eframe::egui;

#[derive(Default)]
pub struct GoToLine {
    visible: bool,
    input: String,
    focus: bool,
}

impl GoToLine {
    pub fn open(&mut self) {
        self.visible = true;
        self.input.clear();
        self.focus = true;
    }

    pub fn render(&mut self, ctx: &egui::Context) -> Option<usize> {
        if !self.visible {
            return None;
        }
        let mut open = self.visible;
        let mut chosen = None;
        let mut close = false;
        egui::Window::new("Go to Line")
            .collapsible(false)
            .resizable(false)
            .fixed_size([360.0, 105.0])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.input)
                        .hint_text("Line number")
                        .desired_width(f32::INFINITY),
                );
                if self.focus {
                    response.request_focus();
                    self.focus = false;
                }
                let (enter, escape) = ui.input(|input| {
                    (
                        input.key_pressed(egui::Key::Enter),
                        input.key_pressed(egui::Key::Escape),
                    )
                });
                if let Ok(line) = self.input.trim().parse::<usize>() {
                    if ui.button("Go").clicked() || enter {
                        chosen = Some(line.max(1));
                        close = true;
                    }
                } else if enter {
                    close = true;
                }
                if escape {
                    close = true;
                }
            });
        self.visible = open && !close;
        chosen
    }
}
