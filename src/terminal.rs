//! A PTY session owns its process and streams through a background supervisor.
use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;

use crate::app::events::{AppEvent, TerminalAction};
use eframe::egui;
use portable_pty::{CommandBuilder, PtySize};

enum TerminalCommand {
    Resize(PtySize),
}
enum TerminalOutput {
    Bytes(Vec<u8>),
    Message(String),
    Exited,
}

pub struct TerminalSession {
    commands: mpsc::SyncSender<TerminalCommand>,
    input: mpsc::SyncSender<Vec<u8>>,
    output: mpsc::Receiver<TerminalOutput>,
    stopped: Arc<AtomicBool>,
    // Retain cleanup ownership through application shutdown. Only a finished
    // supervisor is joined on the UI thread.
    supervisor: Option<std::thread::JoinHandle<()>>,
}
impl TerminalSession {
    pub fn start(root: PathBuf, context: egui::Context) -> Self {
        let (commands, requests) = mpsc::sync_channel(8);
        let (input, input_rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let (output_tx, output) = mpsc::sync_channel(128);
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let supervisor = std::thread::spawn(move || {
            let result = run_session(
                root,
                requests,
                input_rx,
                output_tx.clone(),
                stop,
                context.clone(),
            );
            if let Err(error) = result {
                let _ = output_tx.send(TerminalOutput::Message(error));
            }
            let _ = output_tx.send(TerminalOutput::Exited);
            context.request_repaint();
        });
        Self {
            commands,
            input,
            output,
            stopped,
            supervisor: Some(supervisor),
        }
    }
    fn input(&self, bytes: Vec<u8>) -> Result<(), String> {
        self.input
            .try_send(bytes)
            .map_err(|error| format!("Terminal input was not sent: {error}"))
    }
    fn resize(&self, size: PtySize) -> bool {
        self.commands
            .try_send(TerminalCommand::Resize(size))
            .is_ok()
    }
    pub fn close(&self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
    fn reap(&mut self) -> bool {
        if !self.supervisor.as_ref().is_some_and(std::thread::JoinHandle::is_finished) {
            return false;
        }
        if let Some(supervisor) = self.supervisor.take() { let _ = supervisor.join(); }
        true
    }
}
impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.close();
    }
}

fn run_session(
    root: PathBuf,
    requests: mpsc::Receiver<TerminalCommand>,
    input: mpsc::Receiver<Vec<u8>>,
    output: mpsc::SyncSender<TerminalOutput>,
    stopped: Arc<AtomicBool>,
    context: egui::Context,
) -> Result<(), String> {
    let pair = portable_pty::native_pty_system()
        .openpty(PtySize::default())
        .map_err(|error| error.to_string())?;
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| error.to_string())?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|error| error.to_string())?;
    let shell = std::env::var_os("SHELL")
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| "/bin/zsh".into());
    let mut command = CommandBuilder::new(shell);
    command.arg("-i");
    command.cwd(root);
    command.env("TERM", "xterm-256color");
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| error.to_string())?;
    drop(pair.slave);
    #[cfg(unix)]
    let session_id = child.process_id().and_then(|pid| {
        // SAFETY: getsid only queries a process; portable-pty starts a new session.
        let sid = unsafe { libc::getsid(pid as libc::pid_t) };
        (sid == pid as libc::pid_t).then_some(sid)
    });
    #[cfg(unix)]
    let reader_fd = pair.master.as_raw_fd();
    let reader_finished = Arc::new(AtomicBool::new(false));
    let reader_finish = reader_finished.clone();
    let reader_output = output.clone();
    let reader_context = context.clone();
    let reader_thread = std::thread::spawn(move || {
        let mut bytes = [0; 8192];
        loop {
            #[cfg(unix)]
            if let Some(fd) = reader_fd {
                let mut descriptor = libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                // SAFETY: the supervisor retains the master FD until this thread joins.
                let ready = unsafe { libc::poll(&mut descriptor, 1, 50) };
                if ready == 0 {
                    // After child cleanup, a quiet PTY has no final bytes left.
                    // This also releases readers held open by detached daemons.
                    if reader_finish.load(Ordering::Relaxed) {
                        break;
                    }
                    continue;
                }
                if ready < 0 {
                    if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    break;
                }
            }
            match reader.read(&mut bytes) {
                Ok(0) => break,
                Ok(count) => {
                    if reader_output
                        .send(TerminalOutput::Bytes(bytes[..count].to_vec()))
                        .is_err()
                    {
                        break;
                    }
                    reader_context.request_repaint();
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    let _ = reader_output
                        .try_send(TerminalOutput::Message(format!("Terminal read: {error}")));
                    reader_context.request_repaint();
                    break;
                }
            }
        }
    });
    let writer_stop = stopped.clone();
    let writer_output = output.clone();
    let writer_context = context.clone();
    let writer_thread = std::thread::spawn(move || {
        while !writer_stop.load(Ordering::Relaxed) {
            match input.recv_timeout(Duration::from_millis(50)) {
                Ok(bytes) => {
                    if let Err(error) = writer.write_all(&bytes).and_then(|()| writer.flush()) {
                        let _ = writer_output
                            .try_send(TerminalOutput::Message(format!("Terminal input: {error}")));
                        writer_stop.store(true, Ordering::Relaxed);
                        writer_context.request_repaint();
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    let mut failure = None;
    loop {
        if stopped.load(Ordering::Relaxed) {
            break;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let _ = output.try_send(TerminalOutput::Message(format!("Shell exited: {status}")));
                break;
            }
            Ok(None) => {}
            Err(error) => {
                failure = Some(error.to_string());
                break;
            }
        }
        match requests.recv_timeout(Duration::from_millis(30)) {
            Ok(TerminalCommand::Resize(size)) => {
                if let Err(error) = pair.master.resize(size) {
                    let _ = output
                        .try_send(TerminalOutput::Message(format!("Terminal resize: {error}")));
                    context.request_repaint();
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    stopped.store(true, Ordering::Relaxed);
    #[cfg(unix)]
    if let Some(session_id) = session_id {
        if let Err(error) = terminate_session(session_id) {
            let _ = output.try_send(TerminalOutput::Message(error));
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = writer_thread.join();
    // Keep the master FD valid while the reader drains already queued bytes.
    // The reader stops only on EOF/read error or a quiet poll after cleanup.
    reader_finished.store(true, Ordering::Relaxed);
    let _ = reader_thread.join();
    drop(pair.master);
    failure.map_or(Ok(()), Err)
}

/// Cover every job-control group in the PTY's isolated session, including
/// background, stopped, and reparented children. No shell command is constructed.
#[cfg(unix)]
fn terminate_session(session_id: libc::pid_t) -> Result<(), String> {
    // SAFETY: query only; never target our own session or an invalid session ID.
    if session_id <= 1 || session_id == unsafe { libc::getsid(0) } {
        return Err("Refused to terminate an invalid terminal session".into());
    }
    let mut members = HashSet::new();
    // Stop discovered members before taking another snapshot so ordinary jobs
    // cannot fork more descendants between discovery and termination.
    let mut failure = None;
    for pass in 0..8 {
        let snapshot = std::process::Command::new("/bin/ps")
            .args(["-A", "-o", "pid="])
            .output();
        let snapshot = match snapshot {
            Ok(snapshot) if snapshot.status.success() => snapshot,
            Ok(snapshot) => {
                failure = Some(format!(
                    "Cannot enumerate terminal descendants: {}",
                    snapshot.status
                ));
                break;
            }
            Err(error) => {
                failure = Some(format!("Cannot enumerate terminal descendants: {error}"));
                break;
            }
        };
        let mut discovered = false;
        for pid in String::from_utf8_lossy(&snapshot.stdout)
            .split_whitespace()
            .filter_map(|pid| pid.parse::<libc::pid_t>().ok())
        {
            // SAFETY: only processes still belonging to this isolated session
            // are signalled; this includes jobs with another process group.
            if pid > 1 && unsafe { libc::getsid(pid) } == session_id {
                unsafe {
                    libc::kill(pid, libc::SIGSTOP);
                }
                discovered |= members.insert(pid);
            }
        }
        if !discovered {
            break;
        }
        if pass == 7 {
            failure =
                Some("Terminal descendant enumeration did not settle after eight snapshots".into());
        }
    }
    for pid in members {
        // Recheck session membership before signalling a possibly exited PID.
        if unsafe { libc::getsid(pid) } == session_id {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
    // Also kill the shell group if enumeration failed. Normally every member
    // has already been handled individually, including all background groups.
    unsafe {
        libc::killpg(session_id, libc::SIGKILL);
    }
    failure.map_or(Ok(()), Err)
}

pub struct TerminalTool {
    pub session: Option<TerminalSession>,
    retiring_sessions: Vec<TerminalSession>,
    closing: bool,
    parser: vt100::Parser,
    pub message: String,
    size: PtySize,
    focus_next: bool,
}
impl Default for TerminalTool {
    fn default() -> Self {
        Self {
            session: None,
            retiring_sessions: Vec::new(),
            closing: false,
            parser: vt100::Parser::new(24, 80, 1000),
            message: "Open a terminal in the project directory".into(),
            size: PtySize::default(),
            focus_next: false,
        }
    }
}
impl TerminalTool {
    pub fn has_workers(&self) -> bool {
        self.session.is_some() || !self.retiring_sessions.is_empty()
    }
    pub fn open(&mut self, root: PathBuf, context: egui::Context) {
        self.close();
        if let Some(session) = self.session.take() {
            self.retiring_sessions.push(session);
        }
        self.closing = false;
        self.parser = vt100::Parser::new(24, 80, 1000);
        self.size = PtySize::default();
        self.message = format!("Terminal: {}", root.display());
        self.session = Some(TerminalSession::start(root, context));
        self.focus_next = true;
    }
    pub fn close(&mut self) {
        for session in &self.retiring_sessions { session.close(); }
        if let Some(session) = &self.session {
            session.close();
            self.closing = true;
            self.message = "Closing terminal…".into();
        }
    }
    pub fn send(&mut self, bytes: Vec<u8>) {
        if let Some(session) = self.session.as_ref().filter(|_| !self.closing) {
            if let Err(error) = session.input(bytes) {
                self.message = error;
            }
        }
    }
    pub fn poll(&mut self) {
        // A replacement session gets a clean screen. Keep prior receivers alive
        // and drain them until their supervisor has completed all cleanup.
        self.retiring_sessions.retain_mut(|session| {
            for _ in 0..128 {
                match session.output.try_recv() {
                    Err(_) => break,
                    Ok(_) => {}
                }
            }
            !session.reap()
        });
        let Some(session) = &mut self.session else {
            return;
        };
        // Bound work per frame even for a continuously noisy child.
        for _ in 0..128 {
            let output = match session.output.try_recv() {
                Ok(output) => output,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    break;
                }
            };
            match output {
                TerminalOutput::Bytes(bytes) => self.parser.process(&bytes),
                TerminalOutput::Message(message) => self.message = message,
                TerminalOutput::Exited => {},
            }
        }
        if session.reap() {
            // No producers remain after join; preserve any final queued bytes.
            while let Ok(output) = session.output.try_recv() {
                match output {
                    TerminalOutput::Bytes(bytes) => self.parser.process(&bytes),
                    TerminalOutput::Message(message) => self.message = message,
                    TerminalOutput::Exited => {},
                }
            }
            self.session = None;
            if self.closing {
                self.message = "Terminal closed".into();
            }
            self.closing = false;
        }
    }
}

fn keyboard_bytes(event: &egui::Event) -> Option<Vec<u8>> {
    match event {
        egui::Event::Text(text) | egui::Event::Paste(text) => Some(text.as_bytes().to_vec()),
        egui::Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } => {
            if modifiers.command && !modifiers.ctrl {
                return None;
            }
            if modifiers.ctrl {
                let name = key.name();
                if name.len() == 1 && name.as_bytes()[0].is_ascii_alphabetic() {
                    return Some(vec![name.as_bytes()[0].to_ascii_uppercase() - b'A' + 1]);
                }
            }
            Some(match key {
                egui::Key::Enter => b"\r".to_vec(),
                egui::Key::Backspace => vec![127],
                egui::Key::Tab => b"\t".to_vec(),
                egui::Key::Escape => vec![27],
                egui::Key::ArrowUp => b"\x1b[A".to_vec(),
                egui::Key::ArrowDown => b"\x1b[B".to_vec(),
                egui::Key::ArrowRight => b"\x1b[C".to_vec(),
                egui::Key::ArrowLeft => b"\x1b[D".to_vec(),
                egui::Key::Home => b"\x1b[H".to_vec(),
                egui::Key::End => b"\x1b[F".to_vec(),
                egui::Key::Delete => b"\x1b[3~".to_vec(),
                egui::Key::PageUp => b"\x1b[5~".to_vec(),
                egui::Key::PageDown => b"\x1b[6~".to_vec(),
                _ => return None,
            })
        }
        _ => None,
    }
}

pub fn render(ui: &mut egui::Ui, state: &mut crate::app::AppState) {
    let mut action = None;
    ui.horizontal(|ui| {
        if state.terminal.session.is_none()
            && ui
                .add_enabled(state.project.is_some(), egui::Button::new("Open terminal"))
                .clicked()
        {
            action = Some(TerminalAction::Open);
        }
        if state.terminal.session.is_some() {
            if ui
                .add_enabled(!state.terminal.closing, egui::Button::new("Close session"))
                .clicked()
            {
                action = Some(TerminalAction::Close);
            }
            if ui.button("Interrupt").clicked() {
                action = Some(TerminalAction::Input("\u{3}".into()));
            }
        }
        ui.label(&state.terminal.message);
    });
    let terminal = &mut state.terminal;
    let font = egui::FontId::monospace(13.0);
    let cell_width = ui.fonts_mut(|fonts| fonts.glyph_width(&font, 'M'));
    let line_height = ui.fonts_mut(|fonts| fonts.row_height(&font));
    let area = ui.available_size();
    let size = PtySize {
        rows: (area.y / line_height).floor().clamp(2.0, 200.0) as u16,
        cols: (area.x / cell_width).floor().clamp(10.0, 400.0) as u16,
        pixel_width: 0,
        pixel_height: 0,
    };
    if size != terminal.size {
        if let Some(session) = &terminal.session {
            if session.resize(size) {
                terminal.parser.screen_mut().set_size(size.rows, size.cols);
                terminal.size = size;
            }
        }
    }
    let (rect, response) = ui.allocate_exact_size(
        area.max(egui::vec2(10.0, line_height)),
        egui::Sense::click(),
    );
    if response.clicked() || terminal.focus_next {
        response.request_focus();
        terminal.focus_next = false;
    }
    ui.painter()
        .rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
    let painter = ui.painter().with_clip_rect(rect);
    // VT100 handles CR, ANSI escapes, cursor movement and alternate screens.
    // This first renderer uses a monochrome screen and a visible block cursor.
    painter.text(
        rect.min,
        egui::Align2::LEFT_TOP,
        terminal.parser.screen().contents(),
        font,
        ui.visuals().text_color(),
    );
    if response.has_focus() && terminal.session.is_some() && !terminal.closing {
        ui.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                response.id,
                egui::EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..Default::default()
                },
            )
        });
        let (row, col) = terminal.parser.screen().cursor_position();
        let cursor = egui::Rect::from_min_size(
            rect.min + egui::vec2(col as f32 * cell_width, row as f32 * line_height),
            egui::vec2(cell_width, line_height),
        );
        painter.rect_filled(cursor, 0.0, ui.visuals().text_color().gamma_multiply(0.35));
        let input: Vec<_> =
            ui.input(|input| input.events.iter().filter_map(keyboard_bytes).collect());
        for bytes in input {
            terminal.send(bytes);
        }
    }
    if let Some(action) = action {
        state.enqueue(AppEvent::Terminal(action));
    }
}
