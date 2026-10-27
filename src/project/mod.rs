pub mod discovery;
pub mod gradle;
pub mod gradle_tasks;
pub mod kind;
pub mod maven_cache;
pub mod maven;
pub mod maven_view;
pub mod maven_watch;
pub mod tree;
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
