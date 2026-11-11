use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::discovery::{descriptor_exists, normalize};
use super::{ProjectError, ProjectModule};

pub(crate) fn valid_profile_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with(['!', '-', '?'])
        && !id
            .chars()
            .any(|ch| ch == ',' || ch.is_control() || ch.is_whitespace())
}

#[derive(Default, Deserialize)]
struct Pom {
    #[serde(default)]
    modules: Option<Modules>,
    #[serde(default, rename = "artifactId")]
    artifact_id: Option<String>,
    #[serde(default)]
    profiles: Option<Profiles>,
}

#[derive(Default, Deserialize)]
struct Profiles {
    #[serde(default, rename = "profile")]
    entries: Vec<Profile>,
}

#[derive(Default, Deserialize)]
struct Profile {
    #[serde(default)]
    modules: Option<Modules>,
}

/// Maven Debug Server uses the artifact ID as its project name when evaluating
/// expressions in reactor projects.
pub fn artifact_id(root: &Path) -> Result<Option<String>, ProjectError> {
    let path = root.join("pom.xml");
    let xml = fs::read_to_string(&path).map_err(|source| ProjectError {
        path: path.clone(),
        source,
    })?;
    let pom: Pom = quick_xml::de::from_str(&xml)
        .map_err(|error| ProjectError::invalid(&path, format!("Invalid Maven POM: {error}")))?;
    Ok(pom
        .artifact_id
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty() && !value.contains("${")))
}

#[derive(Default, Deserialize)]
struct Modules {
    #[serde(default, rename = "module")]
    entries: Vec<String>,
}

/// Discover literal module POMs, including those declared by inactive profiles.
/// Property-based module paths require Maven's effective model and are reported.
pub(crate) fn profile_pom_paths(root: &Path, paths: &mut Vec<PathBuf>) -> Result<bool, String> {
    let canonical_root = root.canonicalize().map_err(|e| e.to_string())?;
    let mut pending = vec![root.join("pom.xml")];
    let mut visited = HashSet::new();
    let mut unresolved = false;
    let mut first_error = None;
    while let Some(path) = pending.pop() {
        let parent_inside = path
            .ancestors()
            .find_map(|part| part.canonicalize().ok())
            .is_some_and(|ancestor| ancestor.starts_with(&canonical_root));
        if !parent_inside {
            first_error.get_or_insert_with(|| {
                format!(
                    "Maven module {} is outside the reactor root",
                    path.display()
                )
            });
            continue;
        }
        paths.push(path.clone());
        let identity = match path.canonicalize() {
            Ok(identity) => identity,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                first_error.get_or_insert_with(|| {
                    format!("Cannot inspect Maven module {}: {error}", path.display())
                });
                continue;
            }
        };
        if !identity.starts_with(&canonical_root) {
            first_error.get_or_insert_with(|| {
                format!(
                    "Maven module {} is outside the reactor root",
                    path.display()
                )
            });
            continue;
        }
        if !visited.insert(identity) {
            continue;
        }
        let bytes = match super::maven_cache::read_regular_pom(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                first_error.get_or_insert_with(|| {
                    format!("Cannot read Maven POM {}: {error}", path.display())
                });
                continue;
            }
        };
        let xml = match String::from_utf8(bytes) {
            Ok(xml) => xml,
            Err(error) => {
                first_error.get_or_insert_with(|| {
                    format!("Cannot decode Maven POM {}: {error}", path.display())
                });
                continue;
            }
        };
        let pom: Pom = match quick_xml::de::from_str(&xml) {
            Ok(pom) => pom,
            Err(error) => {
                first_error.get_or_insert_with(|| {
                    format!("Cannot parse Maven POM {}: {error}", path.display())
                });
                continue;
            }
        };
        if let Err(error) = require_complete_document(&xml, &path) {
            first_error.get_or_insert_with(|| error.to_string());
            continue;
        }
        let entries = pom
            .modules
            .into_iter()
            .flat_map(|modules| modules.entries)
            .chain(
                pom.profiles
                    .into_iter()
                    .flat_map(|profiles| profiles.entries)
                    .flat_map(|profile| {
                        profile
                            .modules
                            .into_iter()
                            .flat_map(|modules| modules.entries)
                    }),
            );
        for entry in entries {
            let name = entry.trim();
            if name.is_empty() || name.contains("${") {
                unresolved = true;
                continue;
            }
            let module = normalize(&path.parent().unwrap_or(root).join(name));
            if !module.starts_with(root) {
                first_error.get_or_insert_with(|| {
                    format!(
                        "Maven module {} is outside the reactor root",
                        module.display()
                    )
                });
                continue;
            }
            let is_pom = match fs::metadata(&module) {
                Ok(metadata) => metadata.is_file(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => module
                    .extension()
                    .is_some_and(|extension| extension == "xml"),
                Err(error) => {
                    first_error.get_or_insert_with(|| {
                        format!("Cannot inspect Maven module {}: {error}", module.display())
                    });
                    continue;
                }
            };
            pending.push(if is_pom {
                module
            } else {
                module.join("pom.xml")
            });
        }
    }
    paths.sort();
    paths.dedup();
    match first_error {
        Some(error) => Err(error),
        None => Ok(unresolved),
    }
}

/// Read explicitly declared reactor modules only; no Maven process is launched.
pub fn modules(root: &Path) -> Result<Vec<ProjectModule>, ProjectError> {
    let mut result = Vec::new();
    let mut visited = HashSet::new();
    collect(root, root, &mut visited, &mut result)?;
    Ok(result)
}

fn collect(
    build_root: &Path,
    root: &Path,
    visited: &mut HashSet<std::path::PathBuf>,
    result: &mut Vec<ProjectModule>,
) -> Result<(), ProjectError> {
    let pom_path = root.join("pom.xml");
    // Canonical identity prevents cycles through declarations or symlink aliases.
    let identity = root.canonicalize().map_err(|source| ProjectError {
        path: root.to_path_buf(),
        source,
    })?;
    if !visited.insert(identity) {
        return Ok(());
    }
    let xml = fs::read_to_string(&pom_path).map_err(|source| ProjectError {
        path: pom_path.clone(),
        source,
    })?;
    let pom: Pom = quick_xml::de::from_str(&xml)
        .map_err(|error| ProjectError::invalid(&pom_path, format!("Invalid Maven POM: {error}")))?;
    require_complete_document(&xml, &pom_path)?;
    for entry in pom.modules.unwrap_or_default().entries {
        let name = entry.trim();
        if name.is_empty() || name.contains("${") {
            return Err(ProjectError::invalid(
                &pom_path,
                "Maven module paths must be nonempty literal paths; property expansion is unavailable",
            ));
        }
        let module_root = module_directory(root, name);
        if module_root == build_root || result.iter().any(|module| module.root == module_root) {
            continue;
        }
        let reactor_name = module_root
            .strip_prefix(build_root)
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|_| module_root.to_string_lossy().into_owned());
        result.push(ProjectModule {
            name: reactor_name,
            root: module_root.clone(),
            resolved_root: None,
        });
        // Missing modules remain visible, including modules not checked out yet.
        if descriptor_exists(&module_root.join("pom.xml"))? {
            collect(build_root, &module_root, visited, result)?;
        }
    }
    Ok(())
}

pub(crate) fn module_directory(root: &Path, entry: &str) -> PathBuf {
    let path = normalize(&root.join(entry));
    if path.file_name().is_some_and(|name| name == "pom.xml") && !path.is_dir() {
        path.parent().unwrap_or(root).to_path_buf()
    } else {
        path
    }
}

/// Deserialization may finish after the first root. Consume the entire input
/// separately so a valid leading POM cannot hide a second root or trailing XML
/// syntax errors. XML comments, processing instructions and whitespace may
/// follow the document element.
fn require_complete_document(xml: &str, path: &Path) -> Result<(), ProjectError> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(xml);
    let mut depth = 0usize;
    let mut root_seen = false;
    let mut declaration_seen = false;
    let mut doctype_seen = false;
    loop {
        let event = reader.read_event().map_err(|error| {
            ProjectError::invalid(path, format!("Invalid Maven XML document: {error}"))
        })?;
        match event {
            Event::Start(element) => {
                if depth == 0 {
                    if root_seen || element.local_name().as_ref() != b"project" {
                        return Err(ProjectError::invalid(
                            path,
                            "Maven XML requires one project root",
                        ));
                    }
                    root_seen = true;
                }
                depth += 1;
            }
            Event::Empty(element) if depth == 0 => {
                if root_seen || element.local_name().as_ref() != b"project" {
                    return Err(ProjectError::invalid(
                        path,
                        "Maven XML requires one project root",
                    ));
                }
                root_seen = true;
            }
            Event::End(_) => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    ProjectError::invalid(path, "Unexpected Maven XML closing element")
                })?;
            }
            Event::Text(text) if depth == 0 => {
                if !text
                    .iter()
                    .all(|byte| matches!(*byte, b' ' | b'\t' | b'\r' | b'\n'))
                {
                    return Err(ProjectError::invalid(
                        path,
                        "Unexpected text outside Maven project root",
                    ));
                }
            }
            Event::CData(_) | Event::GeneralRef(_) if depth == 0 => {
                return Err(ProjectError::invalid(
                    path,
                    "Unexpected content outside Maven project root",
                ));
            }
            Event::Decl(_) => {
                if root_seen || declaration_seen || doctype_seen || depth != 0 {
                    return Err(ProjectError::invalid(
                        path,
                        "Unexpected Maven XML declaration",
                    ));
                }
                declaration_seen = true;
            }
            Event::DocType(_) => {
                if root_seen || doctype_seen || depth != 0 {
                    return Err(ProjectError::invalid(
                        path,
                        "Unexpected Maven XML document type",
                    ));
                }
                doctype_seen = true;
            }
            Event::Eof => {
                if !root_seen || depth != 0 {
                    return Err(ProjectError::invalid(path, "Incomplete Maven XML document"));
                }
                return Ok(());
            }
            _ => {}
        }
    }
}
