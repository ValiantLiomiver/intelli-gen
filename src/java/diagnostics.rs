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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TestSummary {
    pub total: u32,
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
}

fn number_after(text: &str, marker: &str) -> Option<u32> {
    let value = text.split_once(marker)?.1.trim_start();
    let digits: String = value.chars().take_while(char::is_ascii_digit).collect();
    (!digits.is_empty()).then(|| digits.parse().ok()).flatten()
}

/// Parse aggregate Maven Surefire and Gradle test-task summaries from console output.
pub fn parse_test_summary(output: &str) -> Option<TestSummary> {
    let plain = plain_text(output);
    let text = plain.trim();
    if text.contains("Tests run:") && !text.contains(" - in ") && !text.contains(" -- in ") {
        let total = number_after(text, "Tests run:")?;
        let failures = number_after(text, "Failures:").unwrap_or(0);
        let errors = number_after(text, "Errors:").unwrap_or(0);
        let skipped = number_after(text, "Skipped:").unwrap_or(0).min(total);
        let failed = failures
            .saturating_add(errors)
            .min(total.saturating_sub(skipped));
        return Some(TestSummary {
            total,
            passed: total.saturating_sub(failed).saturating_sub(skipped),
            failed,
            skipped,
        });
    }

    let marker = if text.contains(" tests completed") {
        " tests completed"
    } else if text.contains(" test completed") {
        " test completed"
    } else {
        return None;
    };
    let total = text
        .split_once(marker)?
        .0
        .split_whitespace()
        .last()?
        .parse::<u32>()
        .ok()?;
    let failed = number_after(text, " failed").unwrap_or(0).min(total);
    let skipped = number_after(text, " skipped")
        .unwrap_or(0)
        .min(total.saturating_sub(failed));
    Some(TestSummary {
        total,
        passed: total.saturating_sub(failed).saturating_sub(skipped),
        failed,
        skipped,
    })
}

/// Maven javac: `[ERROR] /path/File.java:[12,8] message`.
/// Gradle javac: `/path/File.java:12: error: message`.
pub fn parse(root: &Path, output: &str) -> Option<BuildDiagnostic> {
    let clean = plain_text(output);
    let mut text = clean.trim();
    let mut severity = Severity::Error;
    for (prefix, level) in [
        ("[ERROR]", Severity::Error),
        ("[WARNING]", Severity::Warning),
    ] {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest.trim_start();
            severity = level;
            break;
        }
    }
    // Anchor on the source extension, allowing colons and spaces in the path.
    let end = [".java:", ".kt:", ".groovy:"]
        .iter()
        .filter_map(|suffix| text.find(suffix).map(|index| index + suffix.len() - 1))
        .min()?;
    let source = text[..end].trim();
    if source.is_empty() {
        return None;
    }
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
        } else {
            message.strip_prefix("error:").unwrap_or(message).trim()
        };
        (line, message)
    };
    if line == 0 {
        return None;
    }
    let path = PathBuf::from(source);
    Some(BuildDiagnostic {
        path: Some(if path.is_absolute() {
            path
        } else {
            root.join(path)
        }),
        line: Some(line),
        severity,
        message: message.to_owned(),
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
                    if ('@'..='~').contains(&code) {
                        break;
                    }
                }
            }
        } else if character != '\r' {
            output.push(character);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_maven_and_gradle_diagnostics_with_spaces_in_paths() {
        let root = Path::new("/workspace");
        let maven = parse(
            root,
            "[ERROR] /workspace/My App/src/Main.java:[12,8] cannot find symbol",
        )
        .unwrap();
        assert_eq!(
            maven.path.as_deref(),
            Some(Path::new("/workspace/My App/src/Main.java"))
        );
        assert_eq!(maven.line, Some(12));
        assert_eq!(maven.severity, Severity::Error);

        let gradle = parse(
            root,
            "/workspace/My App/src/Main.java:18: warning: unchecked operation",
        )
        .unwrap();
        assert_eq!(
            gradle.path.as_deref(),
            Some(Path::new("/workspace/My App/src/Main.java"))
        );
        assert_eq!(gradle.line, Some(18));
        assert_eq!(gradle.severity, Severity::Warning);
        assert_eq!(gradle.message, "unchecked operation");
    }
}
