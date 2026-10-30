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
    LoadMany {
        request: u64,
        paths: Vec<PathBuf>,
    },
    ReplaceMany {
        request: u64,
        targets: Vec<WorkspaceReplaceTarget>,
        query: String,
        replacement: String,
        case_sensitive: bool,
    },
    Save {
        document: Document,
        destination: Option<PathBuf>,
    },
    CreateJavaClass {
        root: PathBuf,
        directory: PathBuf,
        name: String,
        package: String,
    },
}

#[derive(Clone, Debug)]
pub enum EditorCompletion {
    Loaded {
        requested_path: PathBuf,
        result: Result<Document, String>,
    },
    LoadedMany {
        request: u64,
        result: Result<Vec<Document>, String>,
    },
    ReplacedMany {
        request: u64,
        result: Result<Vec<(Document, usize)>, String>,
    },
    Saved {
        path: PathBuf,
        saved_path: PathBuf,
        snapshot: String,
        result: Result<(), String>,
    },
    JavaClassCreated {
        root: PathBuf,
        directory: PathBuf,
        result: Result<PathBuf, String>,
    },
}

pub struct WorkspaceReplaceTarget {
    pub document: Document,
    pub expected_text: String,
    pub expected_matches: usize,
    pub recheck_disk: bool,
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
                                let document =
                                    Document::load(&path).map_err(|error| error.to_string())?;
                                bytes = bytes
                                    .checked_add(document.text.len())
                                    .ok_or("Workspace edit size overflow")?;
                                if bytes > MAX_WORKSPACE_EDIT_BYTES {
                                    return Err(format!(
                                        "Rename targets exceed the {} MiB workspace edit limit",
                                        MAX_WORKSPACE_EDIT_BYTES / (1024 * 1024)
                                    ));
                                }
                                documents.push(document);
                            }
                            Ok(documents)
                        })();
                        EditorCompletion::LoadedMany { request, result }
                    }
                    IoCommand::ReplaceMany {
                        request,
                        targets,
                        query,
                        replacement,
                        case_sensitive,
                    } => {
                        let result = (|| {
                            if targets.is_empty() || targets.len() > 200 {
                                return Err(
                                    "Select between 1 and 200 project files to replace".into()
                                );
                            }
                            let mut total_bytes = 0usize;
                            let mut replaced = Vec::with_capacity(targets.len());
                            for mut target in targets {
                                if target.document.text != target.expected_text {
                                    return Err(format!(
                                        "{} changed after the replace preview; run the preview again",
                                        target.document.path.display()
                                    ));
                                }
                                if target.recheck_disk {
                                    let current = Document::load(&target.document.path)
                                        .map_err(|error| error.to_string())?;
                                    if current.text != target.expected_text {
                                        return Err(format!(
                                            "{} changed after the replace preview; run the preview again",
                                            target.document.path.display()
                                        ));
                                    }
                                    target.document.text.clone_from(&target.expected_text);
                                }
                                total_bytes = total_bytes
                                    .checked_add(target.document.text.len())
                                    .ok_or("Workspace replace size overflow")?;
                                if total_bytes > MAX_WORKSPACE_EDIT_BYTES {
                                    return Err(format!(
                                        "Workspace replace exceeds the {} MiB limit",
                                        MAX_WORKSPACE_EDIT_BYTES / (1024 * 1024)
                                    ));
                                }
                                let (text, count, first) =
                                    crate::editor::find_replace::replace_all_text(
                                        &target.document.text,
                                        &query,
                                        &replacement,
                                        case_sensitive,
                                    );
                                if count == 0 || count != target.expected_matches {
                                    return Err(format!(
                                        "{} no longer matches the replace preview; run it again",
                                        target.document.path.display()
                                    ));
                                }
                                target.document.text = text;
                                target
                                    .document
                                    .set_cursor(first.unwrap_or(0) + replacement.chars().count());
                                target.document.mark_changed();
                                replaced.push((target.document, count));
                            }
                            Ok(replaced)
                        })();
                        EditorCompletion::ReplacedMany { request, result }
                    }
                    IoCommand::Save {
                        mut document,
                        destination,
                    } => {
                        let old_path = document.path.clone();
                        let result = if let Some(destination) = destination.as_deref() {
                            document.save_as(destination)
                        } else {
                            document.save()
                        }
                        .map_err(|error| error.to_string());
                        EditorCompletion::Saved {
                            path: old_path,
                            saved_path: if result.is_ok() {
                                document.path
                            } else {
                                destination.unwrap_or(document.path)
                            },
                            snapshot: document.text,
                            result,
                        }
                    }
                    IoCommand::CreateJavaClass {
                        root,
                        directory,
                        name,
                        package,
                    } => {
                        let result =
                            document::create_java_class(&root, &directory, &name, &package);
                        EditorCompletion::JavaClassCreated {
                            root,
                            directory,
                            result,
                        }
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
        if paths.len() > 200 {
            return Err("Workspace edit load exceeds the 200-file limit".into());
        }
        self.submit(IoCommand::LoadMany { request, paths })
    }

    pub fn replace_many(
        &self,
        request: u64,
        targets: Vec<WorkspaceReplaceTarget>,
        query: String,
        replacement: String,
        case_sensitive: bool,
    ) -> Result<(), String> {
        if targets.len() > 200 {
            return Err("Workspace replace exceeds the 200-file limit".into());
        }
        self.submit(IoCommand::ReplaceMany {
            request,
            targets,
            query,
            replacement,
            case_sensitive,
        })
    }

    pub fn save(&self, document: Document, destination: Option<PathBuf>) -> Result<(), String> {
        self.submit(IoCommand::Save {
            document,
            destination,
        })
    }

    pub fn create_java_class(
        &self,
        root: PathBuf,
        directory: PathBuf,
        name: String,
        package: String,
    ) -> Result<(), String> {
        self.submit(IoCommand::CreateJavaClass {
            root,
            directory,
            name,
            package,
        })
    }

    pub fn has_workers(&self) -> bool {
        self.pending.load(Ordering::Acquire) > 0
    }

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
#[derive(Clone, Debug)]
pub enum GutterBreakpointAction {
    Toggle(u32),
    SetEnabled { line: u32, enabled: bool },
    SetLogpoint { line: u32, message: String },
}

pub fn editor_gutter(
    ui: &mut eframe::egui::Ui,
    output: &eframe::egui::text_edit::TextEditOutput,
    source: &std::path::Path,
    first_line: u32,
    breakpoints: Option<&[u32]>,
    logpoints: Option<&[u32]>,
    disabled_breakpoints: Option<&[u32]>,
    current_line: Option<u32>,
    diagnostics: &[crate::java::lsp::LanguageDiagnostic],
) -> Option<GutterBreakpointAction> {
    use eframe::egui;
    let mut action = None;
    for (index, row) in output.galley.rows.iter().enumerate() {
        let line = first_line.saturating_add(index as u32);
        let rect = egui::Rect::from_min_max(
            egui::pos2(
                output.response.rect.min.x - 58.0,
                output.galley_pos.y + row.rect().min.y,
            ),
            egui::pos2(
                output.response.rect.min.x - 4.0,
                output.galley_pos.y + row.rect().max.y,
            ),
        );
        if !ui.is_rect_visible(rect) {
            continue;
        }
        let center = egui::pos2(rect.min.x + 8.0, rect.center().y);
        let has_breakpoint = breakpoints.is_some_and(|lines| lines.contains(&line));
        let is_logpoint = logpoints.is_some_and(|lines| lines.contains(&line));
        let is_disabled = disabled_breakpoints.is_some_and(|lines| lines.contains(&line));
        if has_breakpoint && !is_logpoint {
            ui.painter()
                .circle_filled(center, 4.5, egui::Color32::LIGHT_RED);
        } else if is_logpoint {
            ui.painter().rect_filled(
                egui::Rect::from_center_size(center, egui::vec2(8.0, 8.0)),
                1.0,
                egui::Color32::from_rgb(230, 165, 55),
            );
        } else if is_disabled {
            ui.painter().circle_stroke(
                center,
                4.5,
                egui::Stroke::new(1.5, ui.visuals().weak_text_color()),
            );
        }
        let line_diagnostics: Vec<_> = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.range.start.line.saturating_add(1) == line)
            .collect();
        if !line_diagnostics.is_empty() {
            let color = if line_diagnostics.iter().any(|item| item.severity == Some(1)) {
                ui.visuals().error_fg_color
            } else {
                egui::Color32::from_rgb(230, 170, 60)
            };
            ui.painter()
                .circle_filled(egui::pos2(center.x + 18.0, center.y), 3.5, color);
        }
        if current_line == Some(line) {
            ui.painter().text(
                egui::pos2(rect.min.x + 17.0, rect.center().y),
                egui::Align2::CENTER_CENTER,
                "▶",
                egui::FontId::monospace(12.0),
                egui::Color32::YELLOW,
            );
        }
        ui.painter().text(
            rect.right_center(),
            egui::Align2::RIGHT_CENTER,
            line.to_string(),
            egui::FontId::monospace(12.0),
            ui.visuals().weak_text_color(),
        );
        let sense = if breakpoints.is_some() {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        let response = ui.interact(rect, ui.id().with(("gutter", source, line)), sense);
        let mut tooltip = if breakpoints.is_some() {
            String::from("Breakpoint actions")
        } else {
            String::new()
        };
        for diagnostic in line_diagnostics.iter().take(5) {
            let severity = match diagnostic.severity {
                Some(1) => "Error",
                Some(2) => "Warning",
                _ => "Info",
            };
            tooltip.push_str(&format!("\n{severity}: {}", diagnostic.message));
        }
        let response = response.on_hover_text(tooltip);
        let draft_id = ui.id().with(("logpoint-draft", source, line));
        let mut draft = ui
            .data(|data| data.get_temp::<String>(draft_id))
            .unwrap_or_default();
        if breakpoints.is_some() {
            egui::Popup::menu(&response).show(|ui| {
                if is_disabled {
                    if ui
                        .button(crate::i18n::ui_text(ui, "Enable breakpoint"))
                        .clicked()
                    {
                        action = Some(GutterBreakpointAction::SetEnabled {
                            line,
                            enabled: true,
                        });
                        ui.close();
                    }
                } else if is_logpoint {
                    if ui
                        .button(crate::i18n::ui_text(ui, "Remove logpoint"))
                        .clicked()
                    {
                        action = Some(GutterBreakpointAction::Toggle(line));
                        ui.close();
                    }
                    if ui
                        .button(crate::i18n::ui_text(ui, "Disable breakpoint"))
                        .clicked()
                    {
                        action = Some(GutterBreakpointAction::SetEnabled {
                            line,
                            enabled: false,
                        });
                        ui.close();
                    }
                } else {
                    let label = if has_breakpoint {
                        "Remove breakpoint"
                    } else {
                        "Add breakpoint"
                    };
                    if ui.button(label).clicked() {
                        action = Some(GutterBreakpointAction::Toggle(line));
                        ui.close();
                    }
                    if has_breakpoint
                        && ui
                            .button(crate::i18n::ui_text(ui, "Disable breakpoint"))
                            .clicked()
                    {
                        action = Some(GutterBreakpointAction::SetEnabled {
                            line,
                            enabled: false,
                        });
                        ui.close();
                    }
                    ui.separator();
                    ui.label(crate::i18n::ui_text(ui, "Logpoint message"));
                    ui.add(
                        egui::TextEdit::singleline(&mut draft)
                            .desired_width(220.0)
                            .char_limit(4096)
                            .hint_text(crate::i18n::ui_text(ui, "Message with Java expressions")),
                    );
                    if ui
                        .add_enabled(
                            !draft.trim().is_empty(),
                            egui::Button::new(crate::i18n::ui_text(ui, "Add logpoint")),
                        )
                        .clicked()
                    {
                        action = Some(GutterBreakpointAction::SetLogpoint {
                            line,
                            message: draft.clone(),
                        });
                        draft.clear();
                        ui.close();
                    }
                }
                ui.data_mut(|data| data.insert_temp(draft_id, draft));
            });
        }
    }
    action
}

#[cfg(test)]
mod workspace_replace_tests {
    use super::*;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    #[test]
    fn changed_disk_target_rejects_the_whole_workspace_replacement() {
        let root = std::env::temp_dir().join(format!(
            "intelli-gen-replace-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let first = root.join("First.java");
        let second = root.join("Second.java");
        std::fs::write(&first, "class First { String value = \"old\"; }\n").unwrap();
        std::fs::write(&second, "class Second { String value = \"old\"; }\n").unwrap();
        let first_document = Document::load(&first).unwrap();
        let second_document = Document::load(&second).unwrap();
        std::fs::write(&second, "class Second { String value = \"new\"; }\n").unwrap();
        let mut io = EditorIo::new(eframe::egui::Context::default());
        io.replace_many(
            1,
            vec![
                WorkspaceReplaceTarget {
                    expected_text: first_document.text.clone(),
                    document: first_document,
                    expected_matches: 1,
                    recheck_disk: false,
                },
                WorkspaceReplaceTarget {
                    expected_text: second_document.text.clone(),
                    document: second_document,
                    expected_matches: 1,
                    recheck_disk: true,
                },
            ],
            "old".into(),
            "replacement".into(),
            true,
        )
        .unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        let completion = loop {
            if let Some(completion) = io.drain().into_iter().next() {
                break completion;
            }
            assert!(Instant::now() < deadline, "replacement worker timed out");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(matches!(
            completion,
            EditorCompletion::ReplacedMany {
                request: 1,
                result: Err(_)
            }
        ));
        assert_eq!(
            std::fs::read_to_string(&first).unwrap(),
            "class First { String value = \"old\"; }\n"
        );
        assert_eq!(
            std::fs::read_to_string(&second).unwrap(),
            "class Second { String value = \"new\"; }\n"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
