use std::path::PathBuf;

use eframe::egui;

use super::{AppEvent, AppState};

const MAX_VISIBLE_PROBLEMS: usize = 10_000;
const MAX_JAVA_PROBLEMS: usize = 8_000;
const MAX_BUILD_PROBLEMS: usize = 2_000;

#[derive(Clone, Copy)]
enum ProblemOrigin {
    Java(crate::java::lsp::Position),
    Build,
}

struct Problem {
    rank: u8,
    severity: &'static str,
    path: Option<PathBuf>,
    line: u32,
    column: u32,
    source: &'static str,
    message: String,
    origin: ProblemOrigin,
}

fn sort_key(problem: &Problem) -> (u8, Option<&PathBuf>, u32, u32, &str) {
    (
        problem.rank,
        problem.path.as_ref(),
        problem.line,
        problem.column,
        problem.source,
    )
}

pub fn render(ui: &mut egui::Ui, state: &mut AppState) {
    let mut problems = Vec::new();
    for (path, diagnostics) in &state.language.diagnostics {
        for diagnostic in diagnostics
            .iter()
            .take(MAX_JAVA_PROBLEMS.saturating_sub(problems.len()))
        {
            let (rank, severity) = match diagnostic.severity {
                Some(1) => (0, "Error"),
                Some(2) => (1, "Warning"),
                Some(3) => (2, "Information"),
                Some(4) => (3, "Hint"),
                _ => (2, "Information"),
            };
            problems.push(Problem {
                rank,
                severity,
                path: Some(path.clone()),
                line: diagnostic.range.start.line.saturating_add(1),
                column: diagnostic.range.start.character.saturating_add(1),
                source: "Java",
                message: diagnostic.message.clone(),
                origin: ProblemOrigin::Java(diagnostic.range.start),
            });
        }
        if problems.len() >= MAX_VISIBLE_PROBLEMS {
            break;
        }
    }
    for diagnostic in state
        .build
        .diagnostics
        .iter()
        .take(MAX_BUILD_PROBLEMS.min(MAX_VISIBLE_PROBLEMS.saturating_sub(problems.len())))
    {
        let (rank, severity) = match diagnostic.severity {
            crate::java::diagnostics::Severity::Error => (0, "Error"),
            crate::java::diagnostics::Severity::Warning => (1, "Warning"),
        };
        problems.push(Problem {
            rank,
            severity,
            path: diagnostic.path.clone(),
            line: diagnostic.line.unwrap_or(1),
            column: 1,
            source: "Build",
            message: diagnostic.message.clone(),
            origin: ProblemOrigin::Build,
        });
    }
    problems.sort_by(|left, right| sort_key(left).cmp(&sort_key(right)));

    let errors = problems.iter().filter(|problem| problem.rank == 0).count();
    let warnings = problems.iter().filter(|problem| problem.rank == 1).count();
    ui.horizontal(|ui| {
        ui.label(crate::i18n::ui_format(
            ui,
            "{count} displayed",
            &[("count", &problems.len().to_string())],
        ));
        ui.colored_label(
            ui.visuals().error_fg_color,
            crate::i18n::ui_format(ui, "{count} errors", &[("count", &errors.to_string())]),
        );
        ui.colored_label(
            ui.visuals().warn_fg_color,
            crate::i18n::ui_format(ui, "{count} warnings", &[("count", &warnings.to_string())]),
        );
        if problems.len() == MAX_VISIBLE_PROBLEMS {
            ui.small(crate::i18n::ui_format(
                ui,
                "Display limit reached ({count})",
                &[("count", &MAX_VISIBLE_PROBLEMS.to_string())],
            ));
        }
    });
    ui.separator();
    if problems.is_empty() {
        ui.label(crate::i18n::ui_text(
            ui,
            "No Java or build problems reported.",
        ));
        return;
    }
    egui::ScrollArea::both()
        .id_salt("problems_list")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (index, problem) in problems.into_iter().enumerate() {
                let path = problem
                    .path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "Project".into());
                let label = format!(
                    "{}  {}:{}:{}  [{}]  {}",
                    problem.severity,
                    path,
                    problem.line,
                    problem.column,
                    problem.source,
                    problem.message
                );
                let color = match problem.rank {
                    0 => ui.visuals().error_fg_color,
                    1 => ui.visuals().warn_fg_color,
                    _ => ui.visuals().text_color(),
                };
                if ui
                    .push_id(index, |ui| {
                        ui.selectable_label(false, egui::RichText::new(label).color(color))
                    })
                    .inner
                    .clicked()
                    && let Some(path) = problem.path
                {
                    let event = match problem.origin {
                        ProblemOrigin::Java(position) => {
                            AppEvent::OpenFileAtPosition(path, position)
                        }
                        ProblemOrigin::Build => {
                            AppEvent::OpenFileAtLine(path, problem.line as usize)
                        }
                    };
                    state.enqueue(event);
                }
            }
        });
}
