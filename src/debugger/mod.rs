//! Java DAP transport and session ownership, independent of debugger widgets.
pub mod dap;
pub mod session;
pub mod breakpoints;
pub mod stack;
pub mod variables;
pub use stack::DebugController;

use std::fmt;
use std::path::{Path, PathBuf};

pub use session::{DebugClient, DebugEvent, DebugService, DebugSession, DebugState, DebugTool};

#[derive(Clone, Debug)]
pub enum DebugError {
    Configuration(String),
    LanguageService(String),
    Transport(String),
    Adapter { command: String, message: String },
    ApplicationExit(Option<i64>),
    Cancelled,
}
impl fmt::Display for DebugError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(message) => write!(f, "Debugger configuration: {message}"),
            Self::LanguageService(message) => write!(f, "Java debug server: {message}"),
            Self::Transport(message) => write!(f, "DAP connection: {message}"),
            Self::Adapter { command, message } => write!(f, "Debug adapter {command}: {message}"),
            Self::ApplicationExit(code) => write!(f, "Java application exited during debugger startup (exit {code:?})"),
            Self::Cancelled => f.write_str("Debug session cancelled"),
        }
    }
}
impl std::error::Error for DebugError {}
impl From<std::io::Error> for DebugError {
    fn from(error: std::io::Error) -> Self { Self::Transport(error.to_string()) }
}

/// Only explicitly installed bundles are used. Compatibility is checked by
/// JDT LS when it loads the bundle and executes startDebugSession.
pub fn validate_bundle(path: &Path) -> Result<PathBuf, DebugError> {
    use std::io::Read;
    if !path.is_absolute() || !path.is_file() {
        return Err(DebugError::Configuration("Select an existing absolute Java Debug Server plugin JAR path in Settings".into()));
    }
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    if !name.starts_with("com.microsoft.java.debug.plugin") || path.extension().is_none_or(|extension| extension != "jar") {
        return Err(DebugError::Configuration("Select com.microsoft.java.debug.plugin-<version>.jar from an installed java-debug distribution".into()));
    }
    let mut file = std::fs::File::open(path).map_err(|error| DebugError::Configuration(format!("Cannot read {}: {error}", path.display())))?;
    let mut magic = [0; 4];
    file.read_exact(&mut magic).map_err(|error| DebugError::Configuration(format!("Cannot read plugin JAR: {error}")))?;
    if magic != [b'P', b'K', 3, 4] {
        return Err(DebugError::Configuration("The selected Java Debug Server bundle is not a JAR/ZIP archive".into()));
    }
    path.canonicalize().map_err(|error| DebugError::Configuration(error.to_string()))
}

/// UI actions carry the stopped epoch and selection that produced them.
pub fn handle_action(tool: &mut DebugTool, action: crate::app::events::DebuggerAction) {
    use crate::app::events::DebuggerAction as Action;
    use stack::InspectRequest as Request;
    if let Action::ToggleBreakpoint { source, line, modified } = action {
        tool.breakpoints.toggle(source.clone(), line);
        tool.mark_breakpoint_changed(&source);
        if modified { tool.breakpoints.modified.insert(source.clone()); } else { tool.breakpoints.modified.remove(&source); }
        if let Some(controller) = tool.controller() {
            let revision = tool.breakpoints.source_revision(&source);
            tool.inspector.submit(controller, Request::Breakpoints { arguments: tool.breakpoints.arguments(&source), source, revision });
        }
        return;
    }
    if let Action::UpdateBreakpoint { source, line, condition, enabled, modified } = action {
        let condition = condition.filter(|condition| !condition.trim().is_empty());
        if tool.breakpoints.update(&source, line, condition, enabled) {
            tool.mark_breakpoint_changed(&source);
            if modified { tool.breakpoints.modified.insert(source.clone()); }
            else { tool.breakpoints.modified.remove(&source); }
            if let Some(controller) = tool.controller() {
                let revision = tool.breakpoints.source_revision(&source);
                tool.inspector.submit(controller, Request::Breakpoints {
                    arguments: tool.breakpoints.arguments(&source), source, revision,
                });
            }
        }
        return;
    }
    if let Action::RemoveWatch(index) = &action {
        tool.remove_watch(*index);
        return;
    }
    if let Action::AddWatch(expression) = &action {
        let expression = expression.trim().to_owned();
        if expression.is_empty() || tool.watches.contains(&expression) { return; }
        tool.add_watch(expression.clone());
        if let (Some(controller), Some(frame)) = (tool.controller(), tool.inspector.selected_frame) {
            tool.inspector.submit(controller, Request::Evaluate { frame, expression });
        }
        return;
    }
    let Some(controller) = tool.controller() else { return; };
    let request = match action {
        Action::SelectThread { epoch, thread } if epoch == controller.epoch && epoch == tool.inspector.epoch => {
            tool.inspector.select_thread(controller, thread); return;
        }
        Action::SelectFrame { epoch, frame } if epoch == controller.epoch && epoch == tool.inspector.epoch => {
            tool.inspector.select_frame(controller.clone(), frame);
            tool.watched_frame = None;
            return;
        }
        Action::Inspect { epoch, selection, request } if epoch == controller.epoch && selection == tool.inspector.selection => request,
        Action::Continue => { let Some(thread)=tool.inspector.selected_thread else {return;}; Request::Continue(thread) }
        Action::Pause => Request::Pause,
        Action::StepOver => { let Some(thread)=tool.inspector.selected_thread else {return;}; Request::StepOver(thread) }
        Action::StepInto => { let Some(thread)=tool.inspector.selected_thread else {return;}; Request::StepInto(thread) }
        Action::StepOut => { let Some(thread)=tool.inspector.selected_thread else {return;}; Request::StepOut(thread) }
        _ => return,
    };
    if matches!(&request, Request::Continue(_) | Request::Pause | Request::StepOver(_) | Request::StepInto(_) | Request::StepOut(_)) && tool.inspector.control_pending { return; }
    match request {
        Request::Collapse(reference) => { tool.inspector.expanded.remove(&reference); }
        Request::Variables { frame, reference } => tool.inspector.expand(controller, frame, reference),
        request => tool.inspector.submit(controller, request),
    }
}

pub fn render(ui: &mut eframe::egui::Ui, state: &mut crate::app::AppState) {
    use crate::app::events::{AppEvent, DebuggerAction as Action};
    use eframe::egui;
    use stack::InspectRequest as Request;
    let epoch=state.debugger.inspector.epoch;
    let selection=state.debugger.inspector.selection;
    let thread=state.debugger.inspector.selected_thread;
    let ready=state.debugger.controller().is_some_and(|controller| controller.epoch==epoch);
    let stopped=ready && epoch%2==1 && matches!(state.debugger.state,DebugState::Stopped) && !state.debugger.inspector.control_pending;
    let running=ready && epoch%2==0 && matches!(state.debugger.state,DebugState::Running) && !state.debugger.inspector.control_pending;
    let mut actions=Vec::new();
    let error_fg = ui.visuals().error_fg_color;
    ui.horizontal_wrapped(|ui| {
        crate::java::run_config::configuration_selector(ui, &mut state.run);
        let enabled = state.run.selected().is_some() && !state.debugger.has_workers()
            && state.language.tooling.debug_bundle.is_some() && !state.pending_language_restart;
        if ui.add_enabled(enabled, egui::Button::new("Debug")).clicked() { actions.push(Action::Start); }
        if ui.add_enabled(state.debugger.has_workers(), egui::Button::new("Stop debug")).clicked() { actions.push(Action::Stop); }
        if ui.add_enabled(stopped, egui::Button::new("Refresh threads")).clicked() {
            actions.push(Action::Inspect{epoch,selection,request:Request::Threads});
        }
        for (label,request,enabled) in [
            ("Continue",Request::Continue(thread.unwrap_or(0)),stopped && thread.is_some()),
            ("Pause",Request::Pause,running),
            ("Step over",Request::StepOver(thread.unwrap_or(0)),stopped && thread.is_some()),
            ("Step into",Request::StepInto(thread.unwrap_or(0)),stopped && thread.is_some()),
            ("Step out",Request::StepOut(thread.unwrap_or(0)),stopped && thread.is_some()),
        ] {
            if ui.add_enabled(enabled,egui::Button::new(label)).clicked() {actions.push(Action::Inspect{epoch,selection,request});}
        }
    });
    if let Some(error)=&state.language.tooling.debug_error {ui.colored_label(ui.visuals().error_fg_color,error);}
    ui.small("Build the selected configuration before debugging; save source edits to match compiled line numbers.");
    ui.label(&state.debugger.summary);
    if let Some(reason) = &state.debugger.stop_reason {
        ui.small(format!("Paused: {reason}"));
    }
    if !state.debugger.selected_jdk.is_empty() {ui.small(format!("JDK: {}",state.debugger.selected_jdk));}
    if let Some(error)=&state.debugger.inspector.error {ui.colored_label(ui.visuals().error_fg_color,error);}
    if let Some(error)=&state.debugger.breakpoint_storage_error {ui.colored_label(ui.visuals().error_fg_color,error);}
    egui::ScrollArea::both().id_salt("debug_contents").auto_shrink([false,false]).show(ui,|ui| {
        ui.columns(2,|columns| {
            columns[0].strong("Threads and stack");
            for item in &state.debugger.inspector.threads {
                if columns[0].add_enabled(stopped,egui::Button::new(format!("{} [{}]",item.name,item.id)).selected(thread==Some(item.id))).clicked() {
                    actions.push(Action::SelectThread{epoch,thread:item.id});
                }
            }
            for frame in &state.debugger.inspector.frames {
                let selected=state.debugger.inspector.selected_frame==Some(frame.id);
                if columns[0].add_enabled(stopped,egui::Button::new(format!("{} :{}:{}",frame.name,frame.line,frame.column)).selected(selected)).clicked() {
                    actions.push(Action::SelectFrame{epoch,frame:frame.id});
                }
            }
            columns[1].strong("Scopes and variables");
            if let Some(frame)=state.debugger.inspector.selected_frame {
                for (index,scope) in state.debugger.inspector.scopes.iter().enumerate() {
                    columns[1].push_id((epoch,frame,index),|ui| {
                        variable_node(ui,&state.debugger.inspector,&scope.name,scope.variables_reference,frame,0,&mut actions);
                    });
                }
            }
            columns[1].separator();
            columns[1].strong("Watches");
            let watch_response = columns[1].add_enabled(
                stopped && state.debugger.inspector.selected_frame.is_some(),
                egui::TextEdit::singleline(&mut state.debugger.watch_input).hint_text("Java expression"),
            );
            let add_watch = columns[1].add_enabled(
                stopped && state.debugger.inspector.selected_frame.is_some() && !state.debugger.watch_input.trim().is_empty(),
                egui::Button::new("Add watch"),
            ).clicked() || (watch_response.lost_focus() && columns[1].input(|input| input.key_pressed(egui::Key::Enter)));
            if add_watch {
                actions.push(Action::AddWatch(std::mem::take(&mut state.debugger.watch_input)));
            }
            for (index, expression) in state.debugger.watches.iter().enumerate() {
                columns[1].horizontal(|ui| {
                    if ui.small_button("×").clicked() { actions.push(Action::RemoveWatch(index)); }
                    ui.monospace(expression);
                });
                if let Some((_, result)) = state.debugger.inspector.console.iter().rev().find(|(item, _)| item == expression) {
                    match result {
                        Ok(result) => {
                            let label = format!("{}{}", result.result, result.type_name.as_ref().map(|name| format!(" : {name}")).unwrap_or_default());
                            variable_node(
                                &mut columns[1],
                                &state.debugger.inspector,
                                &label,
                                result.variables_reference,
                                state.debugger.inspector.selected_frame.unwrap_or_default(),
                                0,
                                &mut actions,
                            );
                        }
                        Err(error) => {
                            columns[1].colored_label(error_fg, error);
                        }
                    };
                }
            }
        });
        ui.separator();
        egui::CollapsingHeader::new("Source breakpoints").show(ui,|ui| {
            for points in state.debugger.breakpoints.sources.values() {
                for point in points {
                    let source = point.source.clone();
                    let line = point.line;
                    let mut condition = point.condition.clone().unwrap_or_default();
                    let mut enabled = point.enabled;
                    let mut changed = false;
                    let mut remove = false;
                    let status=state.debugger.breakpoints.status.get(&(point.source.clone(),point.line));
                    let label=match status {
                        Some(status) if status.verified=>format!("verified at {}",status.line.unwrap_or(point.line)),
                        Some(status)=>status.message.clone().unwrap_or_else(||"unverified".into()),
                        None=>"pending adapter verification".into(),
                    };
                    let actual=status.and_then(|status|status.source.as_ref()).filter(|source|*source!=&point.source)
                        .map(|source|format!(" in {}",source.display())).unwrap_or_default();
                    let message=status.filter(|status|status.verified).and_then(|status|status.message.as_ref())
                        .map(|message|format!(" — {message}")).unwrap_or_default();
                    ui.push_id((source.clone(), line), |ui| {
                        ui.horizontal(|ui| {
                            changed |= ui.checkbox(&mut enabled, "").on_hover_text("Enable breakpoint").changed();
                            ui.label(format!("{}:{}", source.display(), line));
                            changed |= ui.add(egui::TextEdit::singleline(&mut condition)
                                .desired_width(180.0).hint_text("Condition (optional)"))
                                .on_hover_text("Java expression evaluated when this breakpoint is reached")
                                .changed();
                            remove = ui.small_button("×").on_hover_text("Remove breakpoint").clicked();
                        });
                        ui.small(format!("{}{}{}", label, actual, message));
                    });
                    let modified = state.editor_tabs.iter().any(|tab| tab.path == source && tab.document.is_dirty());
                    if remove {
                        actions.push(Action::ToggleBreakpoint { source, line, modified });
                    } else if changed {
                        actions.push(Action::UpdateBreakpoint {
                            source, line,
                            condition: (!condition.trim().is_empty()).then_some(condition),
                            enabled, modified,
                        });
                    }
                }
            }
        });
        ui.separator();
        ui.strong("Debug Console");
        ui.monospace(&state.debugger.output);
        egui::CollapsingHeader::new(format!("Debugger events ({})", state.debugger.event_log.len()))
            .id_salt("debug_event_log")
            .show(ui, |ui| {
                for event in &state.debugger.event_log { ui.monospace(event); }
            });
        for (index, (expression,result)) in state.debugger.inspector.console.iter().enumerate() {
            ui.push_id(("evaluation", index), |ui| {
                ui.monospace(format!("> {expression}"));
                match result {
                    Ok(result) => {
                        let label = format!("{}{}",result.result,result.type_name.as_ref().map(|name|format!(" : {name}")).unwrap_or_default());
                        variable_node(
                            ui,
                            &state.debugger.inspector,
                            &label,
                            result.variables_reference,
                            state.debugger.inspector.selected_frame.unwrap_or_default(),
                            0,
                            &mut actions,
                        );
                    }
                    Err(error) => {ui.colored_label(ui.visuals().error_fg_color,error);}
                }
            });
        }
        ui.horizontal(|ui| {
            let response=ui.add_enabled(stopped && state.debugger.inspector.selected_frame.is_some(),egui::TextEdit::singleline(&mut state.debugger.inspector.expression).hint_text("Evaluate Java expression in selected frame"));
            let evaluate=ui.add_enabled(stopped && state.debugger.inspector.selected_frame.is_some() && !state.debugger.inspector.expression.trim().is_empty(),egui::Button::new("Evaluate")).clicked()
                || (response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)));
            if evaluate && stopped && !state.debugger.inspector.expression.trim().is_empty() {
                if let Some(frame)=state.debugger.inspector.selected_frame {
                    actions.push(Action::Inspect{epoch,selection,request:Request::Evaluate{frame,expression:std::mem::take(&mut state.debugger.inspector.expression)}});
                    response.request_focus();
                }
            }
        });
    });
    for action in actions {state.enqueue(AppEvent::Debugger(action));}
}

fn variable_node(ui:&mut eframe::egui::Ui, inspector:&stack::DebugInspector, label:&str, reference:i64, frame:i64, depth:usize, actions:&mut Vec<crate::app::events::DebuggerAction>) {
    use crate::app::events::DebuggerAction as Action;
    if reference<=0 {ui.monospace(label);return;}
    if depth>=32 {ui.small("Expansion depth limit reached");return;}
    let expanded=inspector.expanded.contains(&reference);
    ui.horizontal(|ui| {
        if ui.small_button(if expanded {"▾"} else {"▸"}).clicked() {
            if !expanded { actions.push(Action::Inspect{epoch:inspector.epoch,selection:inspector.selection,request:stack::InspectRequest::Variables{frame,reference}}); }
            else { actions.push(Action::Inspect{epoch:inspector.epoch,selection:inspector.selection,request:stack::InspectRequest::Collapse(reference)}); }
        }
        ui.monospace(label);
    });
    if expanded {
        ui.indent((depth,reference,label),|ui| {
            if let Some(variables)=inspector.variables.get(&reference) {
                for (index,variable) in variables.iter().enumerate() {
                    let text=format!("{} = {}{}",variable.name,variable.value,variable.type_name.as_ref().map(|name|format!(" : {name}")).unwrap_or_default());
                    ui.push_id(index,|ui| variable_node(ui,inspector,&text,variable.variables_reference,frame,depth+1,actions));
                }
            } else {ui.small("Loading…");}
        });
    }
}
