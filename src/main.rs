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
const APP_ICON: &[u8] = include_bytes!("../resources/branding/app-icon.png");
const SPLASH: &[u8] = include_bytes!("../resources/branding/splash.png");

fn main() -> eframe::Result {
    let startup_path = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let icon = eframe::icon_data::from_png_bytes(APP_ICON).expect("embedded app icon is valid");
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Intelli-gen")
            .with_icon(icon)
            .with_inner_size([1280.0, 840.0])
            .with_min_inner_size([760.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Intelli-gen",
        options,
        Box::new(move |cc| {
            install_bold_monospace_font(&cc.egui_ctx);
            let image =
                eframe::icon_data::from_png_bytes(SPLASH).expect("embedded splash is valid");
            let splash = cc.egui_ctx.load_texture(
                "startup-splash",
                eframe::egui::ColorImage::from_rgba_unmultiplied(
                    [image.width as usize, image.height as usize],
                    &image.rgba,
                ),
                Default::default(),
            );
            Ok(Box::new(StartupApp {
                context: cc.egui_ctx.clone(),
                startup_path: startup_path.clone(),
                splash,
                splash_frame_shown: false,
                app: None,
            }))
        }),
    )
}

struct StartupApp {
    context: eframe::egui::Context,
    startup_path: Option<std::path::PathBuf>,
    splash: eframe::egui::TextureHandle,
    splash_frame_shown: bool,
    app: Option<app::IdeaLikeApp>,
}

impl eframe::App for StartupApp {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, frame: &mut eframe::Frame) {
        if !self.splash_frame_shown {
            eframe::egui::CentralPanel::default()
                .frame(eframe::egui::Frame::NONE.fill(eframe::egui::Color32::from_rgb(5, 12, 38)))
                .show(ui, |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.add(
                            eframe::egui::Image::new(&self.splash)
                                .fit_to_fraction(eframe::egui::vec2(1.0, 1.0)),
                        );
                    });
                });
            self.splash_frame_shown = true;
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        if self.app.is_none() {
            self.app = Some(app::IdeaLikeApp::new_with_context(
                self.context.clone(),
                self.startup_path.take(),
            ));
        }
        eframe::App::ui(self.app.as_mut().unwrap(), ui, frame);
    }
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
