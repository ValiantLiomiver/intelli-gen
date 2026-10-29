use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use super::discovery::{descriptor_exists, normalize};
use super::{ProjectError, ProjectModule};

#[derive(Default, Deserialize)]
struct Pom {
    #[serde(default)]
    modules: Option<Modules>,
    #[serde(default, rename = "artifactId")]
    artifact_id: Option<String>,
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
        let module_root = normalize(&root.join(name));
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
        });
        // Missing modules remain visible, including modules not checked out yet.
        if descriptor_exists(&module_root.join("pom.xml"))? {
            collect(build_root, &module_root, visited, result)?;
        }
    }
    Ok(())
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
