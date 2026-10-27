use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildDiagnostic {
    pub path: Option<PathBuf>,
    pub line: Option<u32>,
    pub severity: Severity,
    pub message: String,
}

/// Maven javac: `[ERROR] /path/File.java:[12,8] message`.
/// Gradle javac: `/path/File.java:12: error: message`.
pub fn parse(root: &Path, output: &str) -> Option<BuildDiagnostic> {
    let clean = plain_text(output);
    let mut text = clean.trim();
    let mut severity = Severity::Error;
    for (prefix, level) in [("[ERROR]", Severity::Error), ("[WARNING]", Severity::Warning)] {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest.trim_start();
            severity = level;
            break;
        }
    }
    // Anchor on the source extension, allowing colons and spaces in the path.
    let end = [".java:", ".kt:", ".groovy:"].iter()
        .filter_map(|suffix| text.find(suffix).map(|index| index + suffix.len() - 1))
        .min()?;
    let source = text[..end].trim();
    if source.is_empty() { return None; }
    let location = &text[end + 1..];
    let (line, message) = if let Some(location) = location.strip_prefix('[') {
        let (coordinates, message) = location.split_once(']')?;
        let line = coordinates.split(',').next()?.trim().parse::<u32>().ok()?;
        (line, message.trim_start_matches(':').trim())
    } else {
        let (line, message) = location.split_once(':')?;
        let line = line.trim().parse::<u32>().ok()?;
        let message = message.trim();
        let message = if let Some(message) = message.strip_prefix("warning:") {
            severity = Severity::Warning;
            message.trim()
        } else { message.strip_prefix("error:").unwrap_or(message).trim() };
        (line, message)
    };
    if line == 0 { return None; }
    let path = PathBuf::from(source);
    Some(BuildDiagnostic {
        path: Some(if path.is_absolute() { path } else { root.join(path) }),
        line: Some(line), severity, message: message.to_owned(),
    })
}

/// Remove CSI colour/control sequences without requiring a terminal emulator.
pub fn plain_text(input: &str) -> String {
    let mut output = String::new();
    let mut chars = input.chars();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            if chars.next() == Some('[') {
                for code in chars.by_ref() {
                    if ('@'..='~').contains(&code) { break; }
                }
            }
        } else if character != '\r' {
            output.push(character);
        }
    }
    output
}
