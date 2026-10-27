mod app;
mod debugger;
mod editor;
mod git;
mod java;
mod project;
mod search;
mod settings;
mod terminal;

fn main() -> eframe::Result {
    let startup_path = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Intelli-gen — Java Workbench")
            .with_inner_size([1280.0, 840.0])
            .with_min_inner_size([760.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Intelli-gen",
        options,
        Box::new(move |cc| {
            let app = match startup_path.clone() {
                Some(path) => app::IdeaLikeApp::new_with_startup_path(cc, Some(path)),
                None => app::IdeaLikeApp::new(cc),
            };
            Ok(Box::new(app))
        }),
    )
}
