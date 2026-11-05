mod app;
mod debugger;
mod editor;
mod git;
mod i18n;
mod java;
mod project;
mod search;
mod settings;
mod terminal;

const BOLD_MONOSPACE_FAMILY: &str = "intelli-gen-monospace-bold";

fn main() -> eframe::Result {
    let startup_path = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Intelli-gen")
            .with_inner_size([1280.0, 840.0])
            .with_min_inner_size([760.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Intelli-gen",
        options,
        Box::new(move |cc| {
            install_bold_monospace_font(&cc.egui_ctx);
            let app = match startup_path.clone() {
                Some(path) => app::IdeaLikeApp::new_with_startup_path(cc, Some(path)),
                None => app::IdeaLikeApp::new(cc),
            };
            Ok(Box::new(app))
        }),
    )
}

fn install_bold_monospace_font(context: &eframe::egui::Context) {
    use eframe::egui::{
        FontFamily,
        text::{FontData, FontDefinitions},
    };
    use std::sync::Arc;

    let mut fonts = FontDefinitions::default();
    let family = FontFamily::Name(BOLD_MONOSPACE_FAMILY.into());
    fonts.families.insert(
        family.clone(),
        fonts.families[&FontFamily::Monospace].clone(),
    );
    // shortcut: macOS Menlo keeps bold at face index 1; use the regular fallback if unavailable.
    if cfg!(target_os = "macos")
        && let Ok(data) = std::fs::read("/System/Library/Fonts/Menlo.ttc")
    {
        fonts.font_data.insert(
            BOLD_MONOSPACE_FAMILY.into(),
            Arc::new(FontData::from_blob(Arc::new(data), 1)),
        );
        fonts
            .families
            .insert(family, vec![BOLD_MONOSPACE_FAMILY.into()]);
    }
    context.set_fonts(fonts);
}
