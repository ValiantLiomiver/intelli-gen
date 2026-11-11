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
        let root = if kind == ProjectKind::Gradle {
            containing_gradle_build(root)?
        } else {
            root
        };
        let modules = match kind {
            ProjectKind::Maven => maven::modules(root)?,
            ProjectKind::Gradle => gradle::modules(root)?,
            ProjectKind::Unknown => unreachable!(),
        };
        let folder_name = root
            .file_name()
            .unwrap_or(root.as_os_str())
            .to_string_lossy()
            .into_owned();
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
        name: start
            .file_name()
            .unwrap_or(start.as_os_str())
            .to_string_lossy()
            .into_owned(),
        modules: Vec::new(),
        wrapper: None,
    })
}

fn containing_gradle_build(module: &Path) -> Result<&Path, ProjectError> {
    for ancestor in module.ancestors() {
        if descriptor_exists(&ancestor.join("settings.gradle"))?
            || descriptor_exists(&ancestor.join("settings.gradle.kts"))?
        {
            if ancestor != module {
                if gradle::modules(ancestor)?
                    .iter()
                    .any(|entry| entry.root == module)
                {
                    return Ok(ancestor);
                }
                return Err(ProjectError::invalid(
                    module,
                    "This Gradle project is not included in the containing build; add its own settings.gradle or include it in the parent build",
                ));
            }
            break;
        }
    }
    Ok(module)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_nested_folder_preserves_workspace_and_build_roots() {
        let workspace = std::env::temp_dir().join(format!(
            "intelli-gen-discovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let nested = workspace.join("module/src/main/java");
        fs::create_dir_all(&nested).unwrap();
        fs::write(
            workspace.join("pom.xml"),
            "<project><modelVersion>4.0.0</modelVersion><groupId>test</groupId><artifactId>root</artifactId><version>1</version></project>",
        )
        .unwrap();

        let project = discover(&nested);
        assert_eq!(project.root, nested);
        assert_eq!(project.model.unwrap().root, workspace);
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn opening_nested_gradle_folder_preserves_workspace_and_build_roots() {
        let workspace = std::env::temp_dir().join(format!(
            "intelli-gen-gradle-discovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let nested = workspace.join("app/src/main/java");
        fs::create_dir_all(&nested).unwrap();
        fs::write(
            workspace.join("settings.gradle"),
            "rootProject.name = 'sample'\n",
        )
        .unwrap();
        fs::write(workspace.join("build.gradle"), "plugins { id 'java' }\n").unwrap();

        let project = discover(&nested);
        assert_eq!(project.root, nested);
        assert_eq!(project.model.unwrap().root, workspace);
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn included_gradle_module_uses_root_build_and_wrapper() {
        let workspace = std::env::temp_dir().join(format!(
            "intelli-gen-gradle-module-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let nested = workspace.join("app/src/main/java");
        fs::create_dir_all(&nested).unwrap();
        fs::write(workspace.join("settings.gradle"), "include ':app'\n").unwrap();
        fs::write(
            workspace.join("app/build.gradle"),
            "plugins { id 'java' }\n",
        )
        .unwrap();
        let wrapper = workspace.join(if cfg!(windows) {
            "gradlew.bat"
        } else {
            "gradlew"
        });
        fs::write(&wrapper, "wrapper").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let project = discover(&nested);
        assert_eq!(project.root, nested);
        let model = project.model.unwrap();
        assert_eq!(model.root, workspace);
        assert_eq!(model.wrapper.as_deref(), Some(wrapper.as_path()));
        assert_eq!(model.modules[0].name, ":app");
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn standalone_nested_gradle_build_keeps_its_own_root() {
        let workspace = std::env::temp_dir().join(format!(
            "intelli-gen-gradle-independent-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let child = workspace.join("child");
        fs::create_dir_all(&child).unwrap();
        fs::write(workspace.join("settings.gradle"), "include ':other'\n").unwrap();
        fs::write(child.join("build.gradle"), "plugins { id 'java' }\n").unwrap();
        let error = ProjectModel::discover(&child).unwrap_err();
        assert!(error.to_string().contains("not included"));
        fs::write(
            child.join("settings.gradle"),
            "rootProject.name = 'child'\n",
        )
        .unwrap();
        fs::write(workspace.join("settings.gradle"), "include ':child'\n").unwrap();
        assert_eq!(ProjectModel::discover(&child).unwrap().root, child);
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn gradle_module_directory_override_selects_containing_build() {
        let workspace = std::env::temp_dir().join(format!(
            "intelli-gen-gradle-override-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let child = workspace.join("custom/app");
        fs::create_dir_all(&child).unwrap();
        fs::write(
            workspace.join("settings.gradle.kts"),
            "include(\":app\")\nproject(\":app\").projectDir = file(\"custom/app\")\n",
        )
        .unwrap();
        fs::write(child.join("build.gradle.kts"), "plugins { java }\n").unwrap();
        let model = ProjectModel::discover(&child).unwrap();
        assert_eq!(model.root, workspace);
        assert_eq!(model.modules[0].root, child);
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn unsupported_parent_gradle_settings_are_reported_for_nested_build() {
        let workspace = std::env::temp_dir().join(format!(
            "intelli-gen-gradle-dynamic-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let child = workspace.join("app");
        fs::create_dir_all(&child).unwrap();
        fs::write(
            workspace.join("settings.gradle"),
            "include ':app'\ninclude dynamicModules\n",
        )
        .unwrap();
        fs::write(child.join("build.gradle"), "plugins { id 'java' }\n").unwrap();
        assert!(ProjectModel::discover(&child).is_err());
        fs::remove_dir_all(workspace).unwrap();
    }
}
