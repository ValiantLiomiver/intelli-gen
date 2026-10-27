use std::fs;
use std::path::{Component, Path, PathBuf};

use super::{Project, ProjectError, ProjectKind, ProjectModel, gradle, maven};

/// Retain the directory opened by the user even when a containing build is found.
pub fn discover(root: &Path) -> Project {
    let (model, discovery_error) = match ProjectModel::discover(root) {
        Ok(model) => (Some(model), None),
        Err(error) => (None, Some(error.to_string())),
    };
    Project {
        root: root.to_path_buf(),
        kind: model
            .as_ref()
            .map_or(ProjectKind::Unknown, |model| model.kind),
        display_name: root
            .file_name()
            .unwrap_or(root.as_os_str())
            .to_string_lossy()
            .into_owned(),
        model,
        discovery_error,
    }
}

pub(super) fn discover_model(path: &Path) -> Result<ProjectModel, ProjectError> {
    let absolute = std::path::absolute(path).map_err(|source| ProjectError {
        path: path.to_path_buf(),
        source,
    })?;
    let absolute = normalize(&absolute);
    let metadata = fs::metadata(&absolute).map_err(|source| ProjectError {
        path: absolute.clone(),
        source,
    })?;
    let start = if metadata.is_dir() {
        absolute.as_path()
    } else {
        absolute.parent().unwrap_or(&absolute)
    };
    for root in start.ancestors() {
        // Nearest directory wins; Maven breaks ties if both systems are present.
        let kind = if descriptor_exists(&root.join("pom.xml"))? {
            ProjectKind::Maven
        } else if descriptor_exists(&root.join("build.gradle"))?
            || descriptor_exists(&root.join("build.gradle.kts"))?
            || descriptor_exists(&root.join("settings.gradle"))?
            || descriptor_exists(&root.join("settings.gradle.kts"))?
        {
            ProjectKind::Gradle
        } else {
            continue;
        };
        let modules = match kind {
            ProjectKind::Maven => maven::modules(root)?,
            ProjectKind::Gradle => gradle::modules(root)?,
            ProjectKind::Unknown => unreachable!(),
        };
        let folder_name = root.file_name().unwrap_or(root.as_os_str()).to_string_lossy().into_owned();
        let name = if kind == ProjectKind::Maven {
            maven::artifact_id(root)?.unwrap_or(folder_name)
        } else {
            folder_name
        };
        return Ok(ProjectModel {
            root: root.to_path_buf(),
            kind,
            name,
            modules,
            wrapper: wrapper(root, kind)?,
        });
    }
    Ok(ProjectModel {
        root: start.to_path_buf(),
        kind: ProjectKind::Unknown,
        name: start.file_name().unwrap_or(start.as_os_str()).to_string_lossy().into_owned(),
        modules: Vec::new(),
        wrapper: None,
    })
}

pub(super) fn descriptor_exists(path: &Path) -> Result<bool, ProjectError> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(ProjectError::invalid(path, "Expected a regular file")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(ProjectError {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn wrapper(root: &Path, kind: ProjectKind) -> Result<Option<PathBuf>, ProjectError> {
    let name = match kind {
        ProjectKind::Maven if cfg!(windows) => "mvnw.cmd",
        ProjectKind::Maven => "mvnw",
        ProjectKind::Gradle if cfg!(windows) => "gradlew.bat",
        ProjectKind::Gradle => "gradlew",
        ProjectKind::Unknown => return Ok(None),
    };
    let path = root.join(name);
    if !descriptor_exists(&path)? {
        return Ok(None);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(&path).map_err(|source| ProjectError {
            path: path.clone(),
            source,
        })?;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(ProjectError {
                path,
                source: std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Project wrapper is not executable; grant execute permission before building",
                ),
            });
        }
    }
    Ok(Some(path))
}

/// Normalize declared module paths without resolving links or requiring them to exist.
pub(super) fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}
