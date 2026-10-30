use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const MAX_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;
pub const PAGED_EDITOR_BYTES: usize = 256 * 1024;
pub const EDITOR_PAGE_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub struct Document {
    pub path: PathBuf,
    pub text: String,
    pub saved_text: String,
    /// One-based logical position, independent of visual line wrapping.
    pub cursor_line: usize,
    pub cursor_column: usize,
    pub revision: u64,
}

#[derive(Debug)]
pub struct EditorError {
    pub path: PathBuf,
    pub operation: &'static str,
    pub source: io::Error,
}

impl EditorError {
    fn new(path: &Path, operation: &'static str, source: io::Error) -> Self {
        Self {
            path: path.to_owned(),
            operation,
            source,
        }
    }
}

impl fmt::Display for EditorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Cannot {} {}: {}",
            self.operation,
            self.path.display(),
            self.source
        )
    }
}

impl std::error::Error for EditorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl Document {
    /// Synchronous document operations are called exclusively by the editor worker.
    pub fn load(path: &Path) -> Result<Self, EditorError> {
        let normalized = path
            .canonicalize()
            .map_err(|e| EditorError::new(path, "open", e))?;
        let metadata = fs::metadata(&normalized).map_err(|e| EditorError::new(path, "read", e))?;
        if !metadata.is_file() {
            return Err(EditorError::new(
                path,
                "open",
                io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"),
            ));
        }
        // Bound allocation even if the file grows after metadata was read.
        // Oversized/invalid input fails the open, never creates a partial document.
        if metadata.len() > MAX_DOCUMENT_BYTES as u64 {
            return Err(EditorError::new(
                path,
                "open",
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Files larger than 16 MiB are not supported; open this file in an external editor",
                ),
            ));
        }
        let mut text = String::new();
        fs::File::open(&normalized)
            .map_err(|e| EditorError::new(path, "open", e))?
            .take(MAX_DOCUMENT_BYTES as u64 + 1)
            .read_to_string(&mut text)
            .map_err(|e| EditorError::new(path, "read as UTF-8", e))?;
        if text.len() > MAX_DOCUMENT_BYTES {
            return Err(EditorError::new(
                path,
                "open",
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "File grew beyond the 16 MiB editor limit; no partial buffer was opened",
                ),
            ));
        }
        Ok(Self {
            path: normalized,
            saved_text: text.clone(),
            text,
            cursor_line: 1,
            cursor_column: 1,
            revision: 0,
        })
    }

    pub fn is_dirty(&self) -> bool {
        self.text != self.saved_text
    }

    pub fn save(&mut self) -> Result<(), EditorError> {
        atomic_write(&self.path, self.text.as_bytes(), self.saved_text.as_bytes())?;
        self.saved_text.clone_from(&self.text);
        Ok(())
    }

    pub fn save_as(&mut self, destination: &Path) -> Result<(), EditorError> {
        let source = self.path.clone();
        if destination == source {
            return self.save();
        }
        let parent = destination.parent().ok_or_else(|| {
            EditorError::new(
                destination,
                "rename",
                io::Error::new(io::ErrorKind::InvalidInput, "file has no parent directory"),
            )
        })?;
        if parent
            .canonicalize()
            .map_err(|e| EditorError::new(destination, "inspect destination", e))?
            != source
                .parent()
                .unwrap_or(Path::new(""))
                .canonicalize()
                .map_err(|e| EditorError::new(&source, "inspect source directory", e))?
        {
            return Err(EditorError::new(
                destination,
                "rename",
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Java rename must stay in the same directory",
                ),
            ));
        }
        atomic_write_new(
            destination,
            self.text.as_bytes(),
            &source,
            self.saved_text.as_bytes(),
        )?;
        let source_unchanged = match disk_matches(&source, self.saved_text.as_bytes()) {
            Ok(matches) => matches,
            Err(error) => {
                let _ = fs::remove_file(destination);
                return Err(EditorError::new(&source, "rename", error));
            }
        };
        if !source_unchanged {
            let _ = fs::remove_file(destination);
            return Err(external_change_error(&source, "rename"));
        }
        if let Err(error) = fs::remove_file(&source) {
            let _ = fs::remove_file(destination);
            return Err(EditorError::new(
                &source,
                "remove original after rename",
                error,
            ));
        }
        self.path = destination.to_owned();
        self.saved_text.clone_from(&self.text);
        Ok(())
    }

    pub fn set_cursor(&mut self, character_index: usize) {
        self.cursor_line = 1;
        self.cursor_column = 1;
        for character in self.text.chars().take(character_index) {
            if character == '\n' {
                self.cursor_line += 1;
                self.cursor_column = 1;
            } else {
                self.cursor_column += 1;
            }
        }
    }

    pub fn mark_changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

/// Create a public Java class without overwriting an existing file. The
/// selected directory must resolve inside the open workspace.
pub fn create_java_class(
    root: &Path,
    directory: &Path,
    name: &str,
    package: &str,
) -> Result<PathBuf, String> {
    fn identifier(value: &str) -> bool {
        const KEYWORDS: &[&str] = &[
            "abstract",
            "assert",
            "boolean",
            "break",
            "byte",
            "case",
            "catch",
            "char",
            "class",
            "const",
            "continue",
            "default",
            "do",
            "double",
            "else",
            "enum",
            "extends",
            "final",
            "finally",
            "float",
            "for",
            "goto",
            "if",
            "implements",
            "import",
            "instanceof",
            "int",
            "interface",
            "long",
            "native",
            "new",
            "package",
            "private",
            "protected",
            "public",
            "return",
            "short",
            "static",
            "strictfp",
            "super",
            "switch",
            "synchronized",
            "this",
            "throw",
            "throws",
            "transient",
            "try",
            "void",
            "volatile",
            "while",
            "true",
            "false",
            "null",
            "_",
            "var",
            "yield",
            "record",
            "sealed",
            "permits",
        ];
        let mut chars = value.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        (first.is_alphabetic() || first == '_' || first == '$')
            && chars.all(|character| {
                character.is_alphanumeric() || character == '_' || character == '$'
            })
            && !KEYWORDS.contains(&value)
    }

    if !identifier(name) {
        return Err("Class name must be a valid, non-keyword Java identifier".into());
    }
    if !package.is_empty() && !package.split('.').all(identifier) {
        return Err("Package must contain valid Java identifiers separated by dots".into());
    }
    let root = root
        .canonicalize()
        .map_err(|error| format!("Cannot resolve project root: {error}"))?;
    if !root.is_dir() {
        return Err("Project root is not a directory".into());
    }
    let metadata = fs::symlink_metadata(directory).map_err(|error| {
        format!(
            "Cannot inspect destination {}: {error}",
            directory.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("Choose a real project directory, not a symbolic link".into());
    }
    let resolved_directory = directory.canonicalize().map_err(|error| {
        format!(
            "Cannot resolve destination {}: {error}",
            directory.display()
        )
    })?;
    if !resolved_directory.starts_with(&root) {
        return Err("Java classes can only be created inside the open project".into());
    }
    let mut source = String::new();
    if !package.is_empty() {
        source.push_str(&format!("package {package};\n\n"));
    }
    source.push_str(&format!("public class {name} {{\n}}\n"));
    let mut created_directories = Vec::new();
    let result = (|| {
        let mut destination = directory.to_path_buf();
        for segment in package.split('.').filter(|segment| !segment.is_empty()) {
            destination.push(segment);
            match fs::create_dir(&destination) {
                Ok(()) => created_directories.push(destination.clone()),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let metadata = fs::symlink_metadata(&destination).map_err(|error| {
                        format!(
                            "Cannot inspect package directory {}: {error}",
                            destination.display()
                        )
                    })?;
                    if metadata.file_type().is_symlink() || !metadata.is_dir() {
                        return Err(format!(
                            "Package destination must be a real directory: {}",
                            destination.display()
                        ));
                    }
                }
                Err(error) => {
                    return Err(format!(
                        "Cannot create package directory {}: {error}",
                        destination.display()
                    ));
                }
            }
            let resolved = destination.canonicalize().map_err(|error| {
                format!(
                    "Cannot resolve package directory {}: {error}",
                    destination.display()
                )
            })?;
            if !resolved.starts_with(&root) {
                return Err("Java package directories must remain inside the open project".into());
            }
        }
        let path = destination.join(format!("{name}.java"));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| format!("Cannot create {}: {error}", path.display()))?;
        if let Err(error) = file
            .write_all(source.as_bytes())
            .and_then(|()| file.sync_all())
        {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(format!("Cannot write {}: {error}", path.display()));
        }
        Ok(path)
    })();
    if result.is_err() {
        for directory in created_directories.iter().rev() {
            let _ = fs::remove_dir(directory);
        }
    }
    result
}

fn atomic_write(path: &Path, bytes: &[u8], expected: &[u8]) -> Result<(), EditorError> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().ok_or_else(|| {
        EditorError::new(
            path,
            "save",
            io::Error::new(io::ErrorKind::InvalidInput, "file has no parent directory"),
        )
    })?;
    let original =
        fs::metadata(path).map_err(|e| EditorError::new(path, "inspect before saving", e))?;
    // Atomic replacement must not turn a read-only file into a writable one.
    if original.permissions().readonly() {
        return Err(EditorError::new(
            path,
            "save",
            io::Error::new(io::ErrorKind::PermissionDenied, "file is read-only"),
        ));
    }
    let mut temporary = None;
    for _ in 0..128 {
        let mut name = std::ffi::OsString::from(".");
        name.push(path.file_name().unwrap_or_default());
        name.push(format!(
            ".intelli-gen-{}-{}.tmp",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let candidate = parent.join(name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(EditorError::new(
                    path,
                    "create temporary save file for",
                    error,
                ));
            }
        }
    }
    let (temporary_path, mut file) = temporary.ok_or_else(|| {
        EditorError::new(
            path,
            "save",
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "cannot allocate a temporary save file",
            ),
        )
    })?;
    let result = (|| {
        file.set_permissions(original.permissions())?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if !disk_matches(path, expected)? {
            return Err(external_change_error(path, "save").source);
        }
        fs::rename(&temporary_path, path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary_path);
        return Err(EditorError::new(path, "save", error));
    }
    Ok(())
}

fn atomic_write_new(
    path: &Path,
    bytes: &[u8],
    source: &Path,
    expected: &[u8],
) -> Result<(), EditorError> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1_000_000);
    let original =
        fs::metadata(source).map_err(|e| EditorError::new(source, "inspect before renaming", e))?;
    if original.permissions().readonly() {
        return Err(EditorError::new(
            source,
            "rename",
            io::Error::new(io::ErrorKind::PermissionDenied, "file is read-only"),
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        EditorError::new(
            path,
            "rename",
            io::Error::new(io::ErrorKind::InvalidInput, "file has no parent directory"),
        )
    })?;
    for _ in 0..128 {
        let temporary = parent.join(format!(
            ".intelli-gen-{}-{}.tmp",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(EditorError::new(
                    path,
                    "create temporary rename file",
                    error,
                ));
            }
        };
        let result = (|| {
            file.set_permissions(original.permissions())?;
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            if !disk_matches(source, expected)? {
                return Err(external_change_error(source, "rename").source);
            }
            fs::hard_link(&temporary, path)?;
            let _ = fs::remove_file(&temporary);
            Ok(())
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&temporary);
            return Err(EditorError::new(path, "write renamed file", error));
        }
        return Ok(());
    }
    Err(EditorError::new(
        path,
        "rename",
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "cannot allocate temporary rename file",
        ),
    ))
}

fn disk_matches(path: &Path, expected: &[u8]) -> io::Result<bool> {
    let mut current = Vec::with_capacity(expected.len());
    fs::File::open(path)?
        .take(expected.len() as u64 + 1)
        .read_to_end(&mut current)?;
    Ok(current == expected)
}

fn external_change_error(path: &Path, operation: &'static str) -> EditorError {
    EditorError::new(
        path,
        operation,
        io::Error::other(
            "The file changed on disk since it was opened; no changes were saved to protect newer content",
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn save_persists_edits_and_clears_dirty_state() {
        let root = std::env::temp_dir().join(format!(
            "intelli-gen-document-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let path = root.join("Example.java");
        fs::write(&path, "class Example {}\n").unwrap();

        let mut document = Document::load(&path).unwrap();
        document.text = "class Example { int value; }\n".into();
        document.mark_changed();
        assert!(document.is_dirty());
        document.save().unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), document.text);
        assert!(!document.is_dirty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lsp_edits_resolve_utf16_positions_after_non_bmp_characters() {
        let root = std::env::temp_dir().join(format!(
            "intelli-gen-lsp-edit-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let path = root.join("Example.java");
        fs::write(&path, "a😀b\n").unwrap();
        let mut document = Document::load(&path).unwrap();

        document
            .apply_lsp_edits(&[(
                crate::java::lsp::Range {
                    start: crate::java::lsp::Position {
                        line: 0,
                        character: 3,
                    },
                    end: crate::java::lsp::Position {
                        line: 0,
                        character: 4,
                    },
                },
                "B".into(),
            )])
            .unwrap();
        assert_eq!(document.text, "a😀B\n");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_utf16_edit_does_not_partially_change_document() {
        let root = std::env::temp_dir().join(format!(
            "intelli-gen-invalid-lsp-edit-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let path = root.join("Example.java");
        fs::write(&path, "a😀b\n").unwrap();
        let mut document = Document::load(&path).unwrap();
        let before = document.text.clone();

        let result = document.apply_lsp_edits(&[
            (
                crate::java::lsp::Range {
                    start: crate::java::lsp::Position {
                        line: 0,
                        character: 3,
                    },
                    end: crate::java::lsp::Position {
                        line: 0,
                        character: 4,
                    },
                },
                "B".into(),
            ),
            (
                crate::java::lsp::Range {
                    start: crate::java::lsp::Position {
                        line: 0,
                        character: 2,
                    },
                    end: crate::java::lsp::Position {
                        line: 0,
                        character: 3,
                    },
                },
                "x".into(),
            ),
        ]);

        assert!(result.is_err());
        assert_eq!(document.text, before);
        fs::remove_dir_all(root).unwrap();
    }
}

impl Document {
    /// LSP columns count UTF-16 code units; the editor counts Unicode scalars.
    pub fn lsp_position(&self) -> crate::java::lsp::Position {
        let line = self.cursor_line.saturating_sub(1);
        let character = self
            .text
            .split('\n')
            .nth(line)
            .unwrap_or_default()
            .chars()
            .take(self.cursor_column.saturating_sub(1))
            .map(char::len_utf16)
            .sum::<usize>();
        crate::java::lsp::Position {
            line: line as u32,
            character: character as u32,
        }
    }

    pub fn character_index(&self) -> usize {
        self.text
            .split_inclusive('\n')
            .take(self.cursor_line.saturating_sub(1))
            .map(|line| line.chars().count())
            .sum::<usize>()
            + self.cursor_column.saturating_sub(1)
    }

    pub fn word_at_cursor(&self) -> Option<String> {
        let cursor = self.lsp_byte_offset(self.lsp_position())?;
        let line_start = self.text[..cursor]
            .rfind('\n')
            .map_or(0, |newline| newline + 1);
        let line_end = self.text[cursor..]
            .find('\n')
            .map_or(self.text.len(), |newline| cursor + newline);
        let line = &self.text[line_start..line_end];
        let cursor = cursor - line_start;
        let is_identifier_part =
            |character: char| character.is_alphanumeric() || character == '_' || character == '$';
        let start = if line
            .get(cursor..)
            .and_then(|tail| tail.chars().next())
            .is_some_and(is_identifier_part)
        {
            cursor
        } else {
            let (byte, character) = line[..cursor].char_indices().next_back()?;
            is_identifier_part(character).then_some(byte)?
        };
        let mut start = start;
        while start > 0 {
            let Some((byte, character)) = line[..start].char_indices().next_back() else {
                break;
            };
            if !is_identifier_part(character) {
                break;
            }
            start = byte;
        }
        let mut end = cursor.max(start);
        if end == start {
            end += line[start..].chars().next()?.len_utf8();
        }
        while end < line.len() {
            let character = line[end..].chars().next()?;
            if !is_identifier_part(character) {
                break;
            }
            end += character.len_utf8();
        }
        Some(line[start..end].to_owned())
    }

    fn lsp_byte_offset(&self, position: crate::java::lsp::Position) -> Option<usize> {
        let mut start = 0;
        for _ in 0..position.line {
            start += self.text.get(start..)?.find('\n')? + 1;
        }
        let line = self
            .text
            .get(start..)?
            .split('\n')
            .next()?
            .trim_end_matches('\r');
        let mut units = 0;
        for (byte, character) in line.char_indices() {
            if units == position.character as usize {
                return Some(start + byte);
            }
            units += character.len_utf16();
        }
        (units == position.character as usize).then_some(start + line.len())
    }

    pub fn character_index_at_lsp_position(&self, position: crate::java::lsp::Position) -> usize {
        if let Some(byte) = self.lsp_byte_offset(position) {
            return self.text[..byte].chars().count();
        }
        let line_start = self
            .lsp_byte_offset(crate::java::lsp::Position {
                line: position.line,
                character: 0,
            })
            .unwrap_or(self.text.len());
        let line = self.text[line_start..]
            .split('\n')
            .next()
            .unwrap_or_default()
            .trim_end_matches('\r');
        let mut utf16_units = 0;
        let mut scalar_count = 0;
        for character in line.chars() {
            let units = character.len_utf16();
            if utf16_units + units > position.character as usize {
                break;
            }
            utf16_units += units;
            scalar_count += 1;
        }
        self.text[..line_start].chars().count() + scalar_count
    }

    pub fn completion_prefix_range(
        &self,
        position: crate::java::lsp::Position,
    ) -> crate::java::lsp::Range {
        let Some(end) = self.lsp_byte_offset(position) else {
            return crate::java::lsp::Range {
                start: position,
                end: position,
            };
        };
        let prefix_units = self.text[..end]
            .chars()
            .rev()
            .take_while(|character| {
                character.is_alphanumeric() || *character == '_' || *character == '$'
            })
            .map(char::len_utf16)
            .sum::<usize>();
        crate::java::lsp::Range {
            start: crate::java::lsp::Position {
                line: position.line,
                character: position.character.saturating_sub(prefix_units as u32),
            },
            end: position,
        }
    }

    /// Validate all ranges first, then apply from the end. Invalid/overlapping
    /// server edits never leave a partially modified buffer.
    pub fn apply_lsp_edits(
        &mut self,
        edits: &[(crate::java::lsp::Range, String)],
    ) -> Result<(), String> {
        let mut resolved = Vec::new();
        for (index, (range, text)) in edits.iter().enumerate() {
            let start = self
                .lsp_byte_offset(range.start)
                .ok_or("Invalid UTF-16 edit start")?;
            let end = self
                .lsp_byte_offset(range.end)
                .ok_or("Invalid UTF-16 edit end")?;
            if start > end {
                return Err("Reversed server edit range".into());
            }
            resolved.push((start, end, text, index));
        }
        resolved.sort_by_key(|(start, end, _, _)| (*start, *end));
        if resolved
            .windows(2)
            .any(|pair| pair[0].1 > pair[1].0 || pair[0].0 == pair[1].0)
        {
            return Err("Overlapping server edits are unsupported".into());
        }
        let mut cursor_byte = None;
        if let Some(primary) = resolved
            .iter()
            .find(|(_, _, _, index)| *index + 1 == edits.len())
        {
            let mut offset = primary.0 as isize + primary.2.len() as isize;
            for (start, end, text, _) in &resolved {
                if *start < primary.0 {
                    offset += text.len() as isize - (*end - *start) as isize;
                }
            }
            cursor_byte = Some(offset as usize);
        }
        for (start, end, text, _) in resolved.into_iter().rev() {
            self.text.replace_range(start..end, text);
        }
        if !edits.is_empty() {
            self.mark_changed();
        }
        if let Some(byte) = cursor_byte {
            self.set_cursor(self.text[..byte].chars().count());
        }
        Ok(())
    }
}
