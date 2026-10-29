//! Local Git operations, serialized off the UI thread. No remote operations.
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};

use crate::app::events::GitAction;
use eframe::egui;
use git2::{DiffFormat, DiffOptions, ErrorCode, IndexEntry, IndexTime, Repository, StatusOptions};

#[derive(Clone, Debug)]
pub enum GitError {
    Unavailable(String),
    Operation(String),
}
impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(s) | Self::Operation(s) => f.write_str(s),
        }
    }
}
impl std::error::Error for GitError {}
impl From<git2::Error> for GitError {
    fn from(error: git2::Error) -> Self {
        Self::Operation(error.message().into())
    }
}

#[derive(Clone, Debug)]
pub struct ChangedPath {
    pub path: PathBuf,
    pub staged: bool,
    pub unstaged: bool,
    pub conflicted: bool,
}
#[derive(Clone, Debug)]
pub struct GitStatus {
    pub branch: String,
    pub workdir: PathBuf,
    pub changed_paths: Vec<ChangedPath>,
    pub staged_path_count: usize,
}

fn repository(root: &Path) -> Result<Repository, GitError> {
    let repo = Repository::discover(root).map_err(|error| {
        if error.code() == ErrorCode::NotFound {
            GitError::Unavailable("Git unavailable: this project is outside a repository".into())
        } else {
            error.into()
        }
    })?;
    if repo.is_bare() {
        return Err(GitError::Unavailable(
            "Git unavailable: a working tree is required".into(),
        ));
    }
    Ok(repo)
}

impl GitStatus {
    pub fn read(root: &Path) -> Result<Self, GitError> {
        Self::from_repo(&repository(root)?)
    }
    fn from_repo(repo: &Repository) -> Result<Self, GitError> {
        let branch = match repo.head() {
            Ok(head) if head.is_branch() => head.shorthand().unwrap_or("branch").to_owned(),
            Ok(head) => format!(
                "detached {}",
                head.target()
                    .map(|id| id.to_string()[..8].to_owned())
                    .unwrap_or_default()
            ),
            Err(error)
                if error.code() == ErrorCode::UnbornBranch
                    || error.code() == ErrorCode::NotFound =>
            {
                repo.find_reference("HEAD")
                    .ok()
                    .and_then(|head| {
                        head.symbolic_target()
                            .ok()
                            .flatten()
                            .map(|name| name.trim_start_matches("refs/heads/").to_owned())
                    })
                    .unwrap_or_else(|| "unborn branch".into())
            }
            Err(error) => return Err(error.into()),
        };
        let mut options = StatusOptions::new();
        options
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .renames_head_to_index(false)
            .renames_index_to_workdir(false);
        let statuses = repo.statuses(Some(&mut options))?;
        let mut changed_paths = Vec::new();
        let mut staged_path_count = 0;
        for entry in statuses.iter() {
            let flags = entry.status();
            let staged = flags.intersects(
                git2::Status::INDEX_NEW
                    | git2::Status::INDEX_MODIFIED
                    | git2::Status::INDEX_DELETED
                    | git2::Status::INDEX_RENAMED
                    | git2::Status::INDEX_TYPECHANGE,
            );
            staged_path_count += usize::from(staged);
            changed_paths.push(ChangedPath {
                path: bytes_path(entry.path_bytes())?,
                staged,
                unstaged: flags.intersects(
                    git2::Status::WT_NEW
                        | git2::Status::WT_MODIFIED
                        | git2::Status::WT_DELETED
                        | git2::Status::WT_RENAMED
                        | git2::Status::WT_TYPECHANGE,
                ),
                conflicted: flags.is_conflicted(),
            });
        }
        Ok(Self {
            branch,
            workdir: repo.workdir().unwrap_or(repo.path()).to_owned(),
            changed_paths,
            staged_path_count,
        })
    }
}

#[cfg(unix)]
fn bytes_path(bytes: &[u8]) -> Result<PathBuf, GitError> {
    use std::os::unix::ffi::OsStringExt;
    Ok(std::ffi::OsString::from_vec(bytes.to_vec()).into())
}
#[cfg(not(unix))]
fn bytes_path(bytes: &[u8]) -> Result<PathBuf, GitError> {
    std::str::from_utf8(bytes)
        .map(PathBuf::from)
        .map_err(|error| GitError::Operation(error.to_string()))
}
#[cfg(unix)]
fn path_bytes(path: &Path) -> Result<Vec<u8>, GitError> {
    use std::os::unix::ffi::OsStrExt;
    Ok(path.as_os_str().as_bytes().to_vec())
}
#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Result<Vec<u8>, GitError> {
    path.to_str()
        .map(|s| s.as_bytes().to_vec())
        .ok_or_else(|| GitError::Operation("Git path is not UTF-8".into()))
}

fn relative_path(path: &Path) -> Result<&Path, GitError> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(GitError::Operation(
            "Git requires a relative path within the working tree".into(),
        ));
    }
    Ok(path)
}

fn diff(repo: &Repository, path: &Path) -> Result<String, GitError> {
    let path = relative_path(path)?;
    let tree = match repo.head() {
        Ok(head) => Some(head.peel_to_tree()?),
        Err(error) if matches!(error.code(), ErrorCode::UnbornBranch | ErrorCode::NotFound) => None,
        Err(error) => return Err(error.into()),
    };
    let mut text = String::new();
    for staged in [true, false] {
        text.push_str(if staged {
            "--- Staged changes ---\n"
        } else {
            "\n--- Working tree changes ---\n"
        });
        let mut options = DiffOptions::new();
        options
            .pathspec(path)
            .disable_pathspec_match(true)
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .show_untracked_content(true)
            .max_size(16 * 1024 * 1024);
        let changes = if staged {
            repo.diff_tree_to_index(tree.as_ref(), None, Some(&mut options))?
        } else {
            repo.diff_index_to_workdir(None, Some(&mut options))?
        };
        let mut truncated = false;
        let result = changes.print(DiffFormat::Patch, |_delta, _hunk, line| {
            if text.len() + line.content().len() > 2 * 1024 * 1024 {
                truncated = true;
                return false;
            }
            if matches!(line.origin(), '+' | '-' | ' ') {
                text.push(line.origin());
            }
            text.push_str(&String::from_utf8_lossy(line.content()));
            true
        });
        if truncated {
            text.push_str("\nDiff truncated at 2 MiB.\n");
            break;
        }
        result?;
    }
    Ok(text)
}

fn stage(repo: &Repository, path: &Path) -> Result<(), GitError> {
    let path = relative_path(path)?;
    let mut index = repo.index()?;
    let full = repo
        .workdir()
        .ok_or_else(|| GitError::Unavailable("No working tree".into()))?
        .join(path);
    match std::fs::symlink_metadata(full) {
        Ok(_) => index.add_path(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => index.remove_path(path)?,
        Err(error) => return Err(GitError::Operation(error.to_string())),
    }
    index.write()?;
    Ok(())
}

fn unstage(repo: &Repository, path: &Path) -> Result<(), GitError> {
    let path = relative_path(path)?;
    let mut index = repo.index()?;
    let tree = match repo.head() {
        Ok(head) => Some(head.peel_to_tree()?),
        Err(error) if matches!(error.code(), ErrorCode::UnbornBranch | ErrorCode::NotFound) => None,
        Err(error) => return Err(error.into()),
    };
    // Rebuild exactly one index entry, avoiding glob/pathspec interpretation.
    let entry = match tree.as_ref().map(|tree| tree.get_path(path)) {
        Some(Ok(entry)) => Some(entry),
        None => None,
        Some(Err(error)) if error.code() == ErrorCode::NotFound => None,
        Some(Err(error)) => return Err(error.into()),
    };
    if let Some(entry) = entry {
        index.add(&IndexEntry {
            ctime: IndexTime::new(0, 0),
            mtime: IndexTime::new(0, 0),
            dev: 0,
            ino: 0,
            mode: entry.filemode() as u32,
            uid: 0,
            gid: 0,
            file_size: 0,
            id: entry.id(),
            flags: 0,
            flags_extended: 0,
            path: path_bytes(path)?,
        })?;
    } else if let Err(error) = index.remove_path(path) {
        if error.code() != ErrorCode::NotFound {
            return Err(error.into());
        }
    }
    index.write()?;
    Ok(())
}

fn commit(repo: &Repository, message: &str) -> Result<String, GitError> {
    if message.trim().is_empty() {
        return Err(GitError::Operation("Enter a commit message".into()));
    }
    if repo.state() != git2::RepositoryState::Clean {
        return Err(GitError::Operation(
            "Finish the current merge, rebase or other Git operation before committing".into(),
        ));
    }
    let mut index = repo.index()?;
    if index.has_conflicts() {
        return Err(GitError::Operation(
            "Resolve index conflicts before committing".into(),
        ));
    }
    let tree_id = index.write_tree()?;
    let parent = match repo.head() {
        Ok(head) => Some(head.peel_to_commit()?),
        Err(error) if matches!(error.code(), ErrorCode::UnbornBranch | ErrorCode::NotFound) => None,
        Err(error) => return Err(error.into()),
    };
    if parent
        .as_ref()
        .is_some_and(|parent| parent.tree_id() == tree_id)
        || (parent.is_none() && index.is_empty())
    {
        return Err(GitError::Operation("No staged changes to commit".into()));
    }
    let tree = repo.find_tree(tree_id)?;
    let signature = repo.signature()?;
    let parents: Vec<_> = parent.iter().collect();
    Ok(repo
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &parents,
        )?
        .to_string())
}

struct Completion {
    epoch: u64,
    result: Result<GitResult, GitError>,
}
struct GitResult {
    status: GitStatus,
    diff: Option<(PathBuf, String)>,
    committed: bool,
}

pub struct GitTool {
    commands: mpsc::Sender<(u64, PathBuf, GitAction)>,
    completions: mpsc::Receiver<Completion>,
    epoch: Arc<AtomicU64>,
    stopped: Arc<AtomicBool>,
    pub status: Option<GitStatus>,
    pub busy: bool,
    pub message: String,
    pub diff: Option<(PathBuf, String)>,
    pub commit_message: String,
    page: usize,
}
impl GitTool {
    pub fn new(context: egui::Context) -> Self {
        let (commands, requests) = mpsc::channel::<(u64, PathBuf, GitAction)>();
        let (results, completions) = mpsc::channel();
        let epoch = Arc::new(AtomicU64::new(0));
        let current = epoch.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        std::thread::spawn(move || {
            while let Ok((id, root, action)) = requests.recv() {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                if current.load(Ordering::Relaxed) != id {
                    continue;
                }
                let result = (|| {
                    if matches!(&action, GitAction::Refresh) {
                        return Ok(GitResult {
                            status: GitStatus::read(&root)?,
                            diff: None,
                            committed: false,
                        });
                    }
                    let repo = repository(&root)?;
                    let mut selected_diff = None;
                    let mut committed = false;
                    match action {
                        GitAction::Refresh => {}
                        GitAction::Diff(path) => {
                            selected_diff = Some((path.clone(), diff(&repo, &path)?))
                        }
                        GitAction::Stage(path) => stage(&repo, &path)?,
                        GitAction::Unstage(path) => unstage(&repo, &path)?,
                        GitAction::Commit(message) => {
                            commit(&repo, &message)?;
                            committed = true;
                        }
                    }
                    Ok(GitResult {
                        status: GitStatus::from_repo(&repo)?,
                        diff: selected_diff,
                        committed,
                    })
                })();
                if results.send(Completion { epoch: id, result }).is_err() {
                    break;
                }
                context.request_repaint();
            }
        });
        Self {
            commands,
            completions,
            epoch,
            stopped,
            status: None,
            busy: false,
            message: "Open a project to inspect Git status".into(),
            diff: None,
            commit_message: String::new(),
            page: 0,
        }
    }
    pub fn reset(&mut self) {
        self.epoch.fetch_add(1, Ordering::Relaxed);
        self.status = None;
        self.diff = None;
        self.busy = false;
        self.message.clear();
        self.commit_message.clear();
        self.page = 0;
    }
    pub fn dispatch(&mut self, root: PathBuf, action: GitAction) {
        if self.busy {
            return;
        }
        if !matches!(&action, GitAction::Diff(_)) {
            self.diff = None;
        }
        self.busy = self
            .commands
            .send((self.epoch.load(Ordering::Relaxed), root, action))
            .is_ok();
        self.message = if self.busy {
            "Working…"
        } else {
            "Git worker stopped"
        }
        .into();
    }
    pub fn poll(&mut self) {
        for completion in self.completions.try_iter() {
            if completion.epoch != self.epoch.load(Ordering::Relaxed) {
                continue;
            }
            self.busy = false;
            match completion.result {
                Ok(result) => {
                    self.status = Some(result.status);
                    if let Some(diff) = result.diff {
                        self.diff = Some(diff);
                    }
                    self.message = if result.committed {
                        self.commit_message.clear();
                        "Commit created"
                    } else {
                        "Status refreshed"
                    }
                    .into();
                }
                Err(error) => {
                    if matches!(&error, GitError::Unavailable(_)) {
                        self.status = None;
                    }
                    self.message = error.to_string();
                }
            }
        }
    }
}
impl Drop for GitTool {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        self.epoch.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn render(ui: &mut egui::Ui, state: &mut crate::app::AppState) {
    use crate::app::events::AppEvent;
    let mut action = None;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                state.project.is_some() && !state.git.busy,
                egui::Button::new(crate::i18n::ui_text(ui, "Refresh")),
            )
            .clicked()
        {
            action = Some(GitAction::Refresh);
        }
        if let Some(status) = &state.git.status {
            ui.label(crate::i18n::ui_format(
                ui,
                "Branch: {branch}",
                &[("branch", &status.branch)],
            ));
            ui.label(status.workdir.display().to_string());
        }
        ui.label(crate::i18n::message(ui.ctx(), &state.git.message));
    });
    let available = state.git.status.is_some() && !state.git.busy;
    ui.add_enabled_ui(available, |ui| {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut state.git.commit_message).hint_text(
                    crate::i18n::ui_text(ui, "Commit message (commits all staged files)"),
                ),
            );
            let staged = state
                .git
                .status
                .as_ref()
                .is_some_and(|status| status.staged_path_count > 0);
            if ui
                .add_enabled(
                    staged && !state.git.commit_message.trim().is_empty(),
                    egui::Button::new(crate::i18n::ui_text(ui, "Commit staged")),
                )
                .clicked()
            {
                action = Some(GitAction::Commit(state.git.commit_message.clone()));
            }
        });
    });
    const PAGE_SIZE: usize = 100;
    let count = state
        .git
        .status
        .as_ref()
        .map_or(0, |status| status.changed_paths.len());
    let pages = count.div_ceil(PAGE_SIZE).max(1);
    state.git.page = state.git.page.min(pages - 1);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                state.git.page > 0,
                egui::Button::new(crate::i18n::ui_text(ui, "Previous paths")),
            )
            .clicked()
        {
            state.git.page -= 1;
        }
        ui.label(crate::i18n::ui_format(
            ui,
            "Page {page} / {pages} · {count} changed paths · at most {limit} per page",
            &[
                ("page", &(state.git.page + 1).to_string()),
                ("pages", &pages.to_string()),
                ("count", &count.to_string()),
                ("limit", &PAGE_SIZE.to_string()),
            ],
        ));
        if ui
            .add_enabled(
                state.git.page + 1 < pages,
                egui::Button::new(crate::i18n::ui_text(ui, "Next paths")),
            )
            .clicked()
        {
            state.git.page += 1;
        }
    });
    let first = state.git.page * PAGE_SIZE;
    egui::ScrollArea::both()
        .id_salt(("git_content", state.git.page))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if let Some(status) = &state.git.status {
                if status.changed_paths.is_empty() {
                    ui.label(crate::i18n::ui_text(ui, "Working tree clean"));
                }
                let end = first
                    .saturating_add(PAGE_SIZE)
                    .min(status.changed_paths.len());
                let page = status.changed_paths.get(first..end).unwrap_or(&[]);
                for path in page {
                    ui.horizontal(|ui| {
                        let mut flags = Vec::new();
                        if path.staged {
                            flags.push(crate::i18n::ui_text(ui, "Staged"));
                        }
                        if path.unstaged {
                            flags.push(crate::i18n::ui_text(ui, "Modified"));
                        }
                        if path.conflicted {
                            flags.push(crate::i18n::ui_text(ui, "Conflict"));
                        }
                        ui.label(flags.join(" · "));
                        if ui
                            .add_enabled(
                                available,
                                egui::Button::new(path.path.display().to_string()),
                            )
                            .clicked()
                        {
                            action = Some(GitAction::Diff(path.path.clone()));
                        }
                        if ui
                            .add_enabled(
                                available && (path.unstaged || path.conflicted),
                                egui::Button::new(crate::i18n::ui_text(ui, "Stage")),
                            )
                            .clicked()
                        {
                            action = Some(GitAction::Stage(path.path.clone()));
                        }
                        if ui
                            .add_enabled(
                                available && path.staged,
                                egui::Button::new(crate::i18n::ui_text(ui, "Unstage")),
                            )
                            .clicked()
                        {
                            action = Some(GitAction::Unstage(path.path.clone()));
                        }
                    });
                }
            }
            if let Some((path, diff)) = &state.git.diff {
                ui.separator();
                ui.label(path.display().to_string());
                ui.monospace(diff);
            }
        });
    if let Some(action) = action {
        state.enqueue(AppEvent::Git(action));
    }
}
