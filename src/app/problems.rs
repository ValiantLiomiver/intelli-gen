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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProblemCursor {
    path: PathBuf,
    line: u32,
    column: u32,
    source: &'static str,
    message: String,
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

fn navigation_sort_key(problem: &Problem) -> (&PathBuf, u32, u32, &str, &str) {
    (
        problem.path.as_ref().expect("navigable problem path"),
        problem.line,
        problem.column,
        problem.source,
        &problem.message,
    )
}

fn location_key(problem: &Problem) -> (&PathBuf, u32, u32) {
    (
        problem.path.as_ref().expect("navigable problem path"),
        problem.line,
        problem.column,
    )
}

fn collect_problems(state: &AppState) -> Vec<Problem> {
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
    problems
}

pub fn navigate(state: &mut AppState, next: bool) {
    let mut problems = collect_problems(state);
    problems.retain(|problem| problem.path.is_some());
    problems.sort_by(|left, right| navigation_sort_key(left).cmp(&navigation_sort_key(right)));
    problems.dedup_by(|left, right| navigation_sort_key(left) == navigation_sort_key(right));
    if problems.is_empty() {
        state.problem_navigation = None;
        state.status = "No navigable problems reported.".into();
        return;
    }

    let current = state
        .active_tab
        .and_then(|index| state.editor_tabs.get(index))
        .map(|tab| {
            let position = tab.document.lsp_position();
            (
                tab.path.clone(),
                position.line.saturating_add(1),
                position.character.saturating_add(1),
            )
        });
    let selected = state.problem_navigation.as_ref().filter(|selected| {
        current.as_ref().is_some_and(|(path, line, column)| {
            path == &selected.path && *line == selected.line && *column == selected.column
        })
    });
    let anchor = selected.and_then(|selected| {
        problems.iter().position(|problem| {
            problem.path.as_ref() == Some(&selected.path)
                && problem.line == selected.line
                && problem.column == selected.column
                && problem.source == selected.source
                && problem.message == selected.message
        })
    });
    let target = anchor.map_or_else(
        || {
            let current = current.as_ref();
            if next {
                current
                    .and_then(|current| {
                        problems.iter().position(|problem| {
                            location_key(problem) >= (&current.0, current.1, current.2)
                        })
                    })
                    .unwrap_or(0)
            } else {
                current
                    .and_then(|current| {
                        problems.iter().rposition(|problem| {
                            location_key(problem) < (&current.0, current.1, current.2)
                        })
                    })
                    .unwrap_or(problems.len() - 1)
            }
        },
        |index| {
            if next {
                (index + 1) % problems.len()
            } else {
                (index + problems.len() - 1) % problems.len()
            }
        },
    );
    let problem = &problems[target];
    let path = problem
        .path
        .as_ref()
        .expect("navigable problem path")
        .clone();
    state.problem_navigation = Some(ProblemCursor {
        path: path.clone(),
        line: problem.line,
        column: problem.column,
        source: problem.source,
        message: problem.message.clone(),
    });
    let event = match problem.origin {
        ProblemOrigin::Java(position) => AppEvent::OpenFileAtPosition(path, position),
        ProblemOrigin::Build => AppEvent::OpenFileAtLine(path, problem.line as usize),
    };
    state.enqueue(event);
}

pub fn render(ui: &mut egui::Ui, state: &mut AppState) {
    let mut problems = collect_problems(state);
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
                    .unwrap_or_else(|| crate::i18n::ui_text(ui, "Project"));
                let source = if problem.source == "Build" {
                    crate::i18n::ui_text(ui, "Build output")
                } else {
                    crate::i18n::ui_text(ui, problem.source)
                };
                let label = format!(
                    "{}  {}:{}:{}  [{}]  {}",
                    crate::i18n::text(ui.ctx(), problem.severity),
                    path,
                    problem.line,
                    problem.column,
                    source,
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
