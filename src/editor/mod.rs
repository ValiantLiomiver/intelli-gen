pub mod document;
pub mod find_replace;
pub mod highlighter;
pub mod tabs;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub use document::Document;
pub use tabs::EditorTab;

pub const MAX_WORKSPACE_EDIT_BYTES: usize = 128 * 1024 * 1024;

enum IoCommand {
    Load(PathBuf),
    LoadMany { request: u64, paths: Vec<PathBuf> },
    Save { document: Document, destination: Option<PathBuf> },
    CreateJavaClass { root: PathBuf, directory: PathBuf, name: String, package: String },
}

#[derive(Clone, Debug)]
pub enum EditorCompletion {
    Loaded {
        requested_path: PathBuf,
        result: Result<Document, String>,
    },
    LoadedMany { request: u64, result: Result<Vec<Document>, String> },
    Saved {
        path: PathBuf,
        saved_path: PathBuf,
        snapshot: String,
        result: Result<(), String>,
    },
    JavaClassCreated { root: PathBuf, directory: PathBuf, result: Result<PathBuf, String> },
}

/// One serial worker orders saves and never blocks the UI on filesystem access.
pub struct EditorIo {
    commands: Sender<IoCommand>,
    completions: Receiver<EditorCompletion>,
    stopped: Arc<AtomicBool>,
    pending: Arc<std::sync::atomic::AtomicUsize>,
}

impl EditorIo {
    pub fn new(context: eframe::egui::Context) -> Self {
        let (commands, requests) = mpsc::channel();
        let (results, completions) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = stopped.clone();
        let pending = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let worker_pending = pending.clone();
        std::thread::spawn(move || {
            while let Ok(command) = requests.recv() {
                if worker_stopped.load(Ordering::Relaxed) {
                    break;
                }
                let completion = match command {
                    IoCommand::Load(requested_path) => {
                        let result =
                            Document::load(&requested_path).map_err(|error| error.to_string());
                        EditorCompletion::Loaded {
                            requested_path,
                            result,
                        }
                    }
                    IoCommand::LoadMany { request, paths } => {
                        let result = (|| {
                            let mut documents = Vec::with_capacity(paths.len());
                            let mut bytes = 0usize;
                            for path in paths {
                                let document = Document::load(&path).map_err(|error| error.to_string())?;
                                bytes = bytes.checked_add(document.text.len()).ok_or("Workspace edit size overflow")?;
                                if bytes > MAX_WORKSPACE_EDIT_BYTES {
                                    return Err(format!("Rename targets exceed the {} MiB workspace edit limit", MAX_WORKSPACE_EDIT_BYTES / (1024 * 1024)));
                                }
                                documents.push(document);
                            }
                            Ok(documents)
                        })();
                        EditorCompletion::LoadedMany { request, result }
                    }
                    IoCommand::Save { mut document, destination } => {
                        let old_path = document.path.clone();
                        let result = if let Some(destination) = destination.as_deref() {
                            document.save_as(destination)
                        } else { document.save() }.map_err(|error| error.to_string());
                        EditorCompletion::Saved {
                            path: old_path,
                            saved_path: if result.is_ok() { document.path } else { destination.unwrap_or(document.path) },
                            snapshot: document.text,
                            result,
                        }
                    }
                    IoCommand::CreateJavaClass { root, directory, name, package } => {
                        let result = document::create_java_class(&root, &directory, &name, &package);
                        EditorCompletion::JavaClassCreated { root, directory, result }
                    }
                };
                if results.send(completion).is_err() {
                    worker_pending.fetch_sub(1, Ordering::AcqRel);
                    break;
                }
                worker_pending.fetch_sub(1, Ordering::AcqRel);
                context.request_repaint();
            }
        });
        Self {
            commands,
            completions,
            stopped,
            pending,
        }
    }

    fn submit(&self, command: IoCommand) -> Result<(), String> {
        self.pending.fetch_add(1, Ordering::AcqRel);
        if self.commands.send(command).is_err() {
            self.pending.fetch_sub(1, Ordering::AcqRel);
            return Err("The editor file worker stopped".into());
        }
        Ok(())
    }

    pub fn load(&self, path: PathBuf) -> Result<(), String> {
        self.submit(IoCommand::Load(path))
    }

    pub fn load_many(&self, request: u64, paths: Vec<PathBuf>) -> Result<(), String> {
        if paths.len() > 200 { return Err("Workspace edit load exceeds the 200-file limit".into()); }
        self.submit(IoCommand::LoadMany { request, paths })
    }

    pub fn save(&self, document: Document, destination: Option<PathBuf>) -> Result<(), String> {
        self.submit(IoCommand::Save { document, destination })
    }

    pub fn create_java_class(&self, root: PathBuf, directory: PathBuf, name: String, package: String) -> Result<(), String> {
        self.submit(IoCommand::CreateJavaClass { root, directory, name, package })
    }

    pub fn has_workers(&self) -> bool { self.pending.load(Ordering::Acquire) > 0 }

    pub fn drain(&self) -> Vec<EditorCompletion> {
        self.completions.try_iter().collect()
    }
}

impl Drop for EditorIo {
    fn drop(&mut self) {
        // Cancel queued work without joining a potentially blocked filesystem call.
        // Any write already in progress finishes its atomic replacement.
        self.stopped.store(true, Ordering::Relaxed);
    }
}

/// Paint the gutter against TextEdit's actual galley rows, inside the same
/// scroll area as the source, so markers follow both scroll directions.
pub fn editor_gutter(
    ui: &mut eframe::egui::Ui,
    output: &eframe::egui::text_edit::TextEditOutput,
    source: &std::path::Path,
    first_line: u32,
    breakpoints: Option<&[u32]>,
    current_line: Option<u32>,
    diagnostics: &[crate::java::lsp::LanguageDiagnostic],
) -> Option<u32> {
    use eframe::egui;
    let mut toggled = None;
    for (index, row) in output.galley.rows.iter().enumerate() {
        let line = first_line.saturating_add(index as u32);
        let rect = egui::Rect::from_min_max(
            egui::pos2(output.response.rect.min.x - 58.0, output.galley_pos.y + row.rect().min.y),
            egui::pos2(output.response.rect.min.x - 4.0, output.galley_pos.y + row.rect().max.y),
        );
        if !ui.is_rect_visible(rect) { continue; }
        let center = egui::pos2(rect.min.x + 8.0, rect.center().y);
        if breakpoints.is_some_and(|lines| lines.contains(&line)) {
            ui.painter().circle_filled(center, 4.5, egui::Color32::LIGHT_RED);
        }
        let line_diagnostics: Vec<_> = diagnostics.iter()
            .filter(|diagnostic| diagnostic.range.start.line.saturating_add(1) == line)
            .collect();
        if !line_diagnostics.is_empty() {
            let color = if line_diagnostics.iter().any(|item| item.severity == Some(1)) {
                ui.visuals().error_fg_color
            } else {
                egui::Color32::from_rgb(230, 170, 60)
            };
            ui.painter().circle_filled(egui::pos2(center.x + 18.0, center.y), 3.5, color);
        }
        if current_line == Some(line) {
            ui.painter().text(egui::pos2(rect.min.x+17.0,rect.center().y),egui::Align2::CENTER_CENTER,"▶",egui::FontId::monospace(12.0),egui::Color32::YELLOW);
        }
        ui.painter().text(rect.right_center(),egui::Align2::RIGHT_CENTER,line.to_string(),egui::FontId::monospace(12.0),ui.visuals().weak_text_color());
        if breakpoints.is_some() || !line_diagnostics.is_empty() {
            let sense = if breakpoints.is_some() { egui::Sense::click() } else { egui::Sense::hover() };
            let response = ui.interact(rect, egui::Id::new(("gutter", source, line)), sense);
            let mut tooltip = if breakpoints.is_some() {
                String::from("Toggle source breakpoint")
            } else {
                String::new()
            };
            for diagnostic in line_diagnostics.iter().take(5) {
                let severity = match diagnostic.severity { Some(1) => "Error", Some(2) => "Warning", _ => "Info" };
                tooltip.push_str(&format!("\n{severity}: {}", diagnostic.message));
            }
            if response.on_hover_text(tooltip).clicked() && breakpoints.is_some() { toggled=Some(line); }
        }
    }
    toggled
}
