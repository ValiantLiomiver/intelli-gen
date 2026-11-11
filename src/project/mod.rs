pub mod discovery;
pub mod editor_config;
pub mod gradle;
pub mod gradle_tasks;
pub mod kind;
pub mod maven;
pub mod maven_cache;
pub mod maven_view;
pub mod maven_watch;
pub mod tree;
pub mod tree_watch;
pub mod workspace;

use std::fmt;
use std::path::{Path, PathBuf};

pub use kind::ProjectKind;

#[derive(Clone, Debug)]
pub struct Project {
    /// The directory the user opened, independent of any selected Java module.
    pub root: PathBuf,
    pub kind: ProjectKind,
    pub display_name: String,
    /// Build services use this root; file tools continue to use `root` above.
    pub model: Option<ProjectModel>,
    pub discovery_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectModule {
    /// Maven reactor path or Gradle project path (for example `:app`).
    pub name: String,
    pub root: PathBuf,
    /// Existing module's physical path, when it differs from its declared path.
    pub resolved_root: Option<PathBuf>,
}

impl ProjectModule {
    pub fn identity_root(&self) -> &Path {
        self.resolved_root.as_deref().unwrap_or(&self.root)
    }
}

#[derive(Clone, Debug)]
pub struct ProjectModel {
    pub root: PathBuf,
    pub kind: ProjectKind,
    /// Build-system project name used by language/debug tooling.
    pub name: String,
    pub modules: Vec<ProjectModule>,
    pub wrapper: Option<PathBuf>,
}

impl ProjectModel {
    pub fn discover(path: &Path) -> Result<Self, ProjectError> {
        discovery::discover_model(path)
    }
}

pub(crate) fn resolve_program_path(root: &Path, program: &Path) -> PathBuf {
    if program.is_absolute() {
        program.to_path_buf()
    } else if program.components().count() > 1 {
        root.join(program)
    } else {
        program.to_path_buf()
    }
}

pub(crate) struct ScratchDirectory(pub PathBuf);

impl ScratchDirectory {
    pub(crate) fn create(
        tool: &str,
        create_error: impl Fn(&Path, std::io::Error) -> String,
        exhausted_error: &str,
    ) -> Result<Self, String> {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};

        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..16 {
            let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "intelli-gen-{tool}-{}-{timestamp}-{sequence}",
                std::process::id()
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(create_error(&path, error)),
            }
        }
        Err(exhausted_error.into())
    }
}

impl std::ops::Deref for ScratchDirectory {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug)]
pub struct ProjectError {
    pub path: PathBuf,
    pub source: std::io::Error,
}

impl fmt::Display for ProjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Project error at {}: {}",
            self.path.display(),
            self.source
        )
    }
}

impl std::error::Error for ProjectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl ProjectError {
    pub(crate) fn invalid(path: &Path, message: impl Into<String>) -> Self {
        Self {
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, message.into()),
        }
    }
}
