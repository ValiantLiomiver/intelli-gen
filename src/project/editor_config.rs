use std::{fs, path::{Path, PathBuf}};

use globset::{GlobBuilder, GlobMatcher};

const MAX_CONFIG_BYTES: u64 = 256 * 1024;
const MAX_CONFIG_FILES: usize = 256;
const MAX_SCANNED_DIRECTORIES: usize = 50_000;

#[derive(Clone, Copy, Debug, Default)]
pub struct EditorStyle {
    /// Outer `None` means inherit; inner `None` means EditorConfig `unset`.
    pub indent_size: Option<Option<usize>>,
    pub insert_spaces: Option<Option<bool>>,
}

#[derive(Clone, Debug)]
struct Rule {
    matcher: GlobMatcher,
    style: EditorStyle,
}

#[derive(Clone, Debug, Default)]
struct ParsedConfig {
    root: bool,
    base: EditorStyle,
    rules: Vec<Rule>,
}

#[derive(Clone, Debug)]
struct ConfigFile {
    directory: PathBuf,
    parsed: ParsedConfig,
}

#[derive(Clone, Debug, Default)]
pub struct ProjectEditorConfig {
    files: Vec<ConfigFile>,
}

impl ProjectEditorConfig {
    /// Scan and parse configuration on the project-open worker, never on the UI thread.
    pub fn load(root: &Path) -> Self {
        let mut config = Self::default();
        let root = root.canonicalize().unwrap_or_else(|_| root.to_owned());
        let mut ancestors = Vec::new();
        let mut directory = root.parent();
        for _ in 0..32 {
            let Some(current) = directory else { break; };
            let path = current.join(".editorconfig");
            if let Some(parsed) = read_config(&path) {
                let stop = parsed.root;
                ancestors.push(ConfigFile { directory: current.to_owned(), parsed });
                if stop { break; }
            }
            directory = current.parent();
        }
        config.files.extend(ancestors.into_iter().rev());
        let mut scanned = 0;
        scan_directory(&root, Path::new(""), 0, &mut scanned, &mut config.files);
        config.files.sort_by_key(|file| file.directory.components().count());
        config
    }

    pub fn style_for(&self, root: &Path, file: &Path) -> EditorStyle {
        if !file.starts_with(root) { return EditorStyle::default(); }
        let mut applicable: Vec<_> = self.files.iter()
            .filter(|config| file.starts_with(&config.directory))
            .collect();
        if let Some(root_config) = applicable.iter().rposition(|config| config.parsed.root) {
            applicable.drain(..root_config);
        }
        let mut resolved = EditorStyle::default();
        for config in applicable {
            let Ok(local_path) = file.strip_prefix(&config.directory) else { continue; };
            let section_style = config.parsed.style_for(local_path);
            overlay(&mut resolved, section_style);
        }
        resolved
    }
}

fn scan_directory(root: &Path, relative: &Path, depth: usize, scanned: &mut usize, files: &mut Vec<ConfigFile>) {
    if depth > 64 || *scanned >= MAX_SCANNED_DIRECTORIES || files.len() >= MAX_CONFIG_FILES { return; }
    *scanned += 1;
    let directory = root.join(relative);
    let Ok(entries) = fs::read_dir(directory) else { return; };
    for entry in entries.flatten() {
        if files.len() >= MAX_CONFIG_FILES || *scanned >= MAX_SCANNED_DIRECTORIES { break; }
        let name = entry.file_name();
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else { continue; };
        if metadata.file_type().is_symlink() { continue; }
        if metadata.is_dir() {
            if matches!(name.to_str(), Some(".git" | "target" | "build" | ".gradle" | ".idea" | "out" | "node_modules")) { continue; }
            let child = if relative.as_os_str().is_empty() { PathBuf::from(name) } else { relative.join(name) };
            scan_directory(root, &child, depth + 1, scanned, files);
        } else if name == ".editorconfig" && metadata.is_file() && metadata.len() <= MAX_CONFIG_BYTES {
            if let Some(parsed) = read_config(&path) {
                files.push(ConfigFile { directory: root.join(relative), parsed });
            }
        }
    }
}

fn read_config(path: &Path) -> Option<ParsedConfig> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES { return None; }
    let text = fs::read_to_string(path).ok()?;
    Some(ParsedConfig::parse(&text))
}

fn overlay(target: &mut EditorStyle, source: EditorStyle) {
    if source.indent_size.is_some() { target.indent_size = source.indent_size; }
    if source.insert_spaces.is_some() { target.insert_spaces = source.insert_spaces; }
}

impl ParsedConfig {
    fn parse(text: &str) -> Self {
        let mut config = Self::default();
        let mut current: Option<Rule> = None;
        let mut base_tab_width = 4usize;
        let mut current_tab_width = base_tab_width;
        let mut base_indent_is_tab = false;
        let mut current_indent_is_tab = false;
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') { continue; }
            if line.starts_with('[') && line.ends_with(']') {
                if let Some(rule) = current.take() { config.rules.push(rule); }
                current_tab_width = base_tab_width;
                current_indent_is_tab = false;
                let pattern = &line[1..line.len() - 1];
                current = GlobBuilder::new(pattern).literal_separator(false).build().ok()
                    .map(|glob| Rule { matcher: glob.compile_matcher(), style: EditorStyle::default() });
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue; };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim().to_ascii_lowercase();
            if key == "root" && current.is_none() {
                config.root = value == "true";
                continue;
            }
            if key == "tab_width" {
                if let Ok(size) = value.parse::<usize>() {
                    if (1..=16).contains(&size) {
                        if let Some(rule) = current.as_mut() {
                            current_tab_width = size;
                            if current_indent_is_tab { rule.style.indent_size = Some(Some(size)); }
                        } else {
                            base_tab_width = size;
                            if base_indent_is_tab { config.base.indent_size = Some(Some(size)); }
                        }
                    }
                }
            }
            let in_section = current.is_some();
            let target = current.as_mut().map(|rule| &mut rule.style).unwrap_or(&mut config.base);
            match key.as_str() {
                "indent_style" if value == "space" => target.insert_spaces = Some(Some(true)),
                "indent_style" if value == "tab" => target.insert_spaces = Some(Some(false)),
                "indent_style" if value == "unset" => target.insert_spaces = Some(None),
                "indent_size" if value == "tab" => {
                    target.indent_size = Some(Some(if in_section { current_tab_width } else { base_tab_width }));
                    if in_section { current_indent_is_tab = true; } else { base_indent_is_tab = true; }
                }
                "indent_size" if value == "unset" => {
                    target.indent_size = Some(None);
                    if in_section { current_indent_is_tab = false; } else { base_indent_is_tab = false; }
                }
                "indent_size" => if let Ok(size) = value.parse::<usize>() {
                    if (1..=16).contains(&size) {
                        target.indent_size = Some(Some(size));
                        if in_section { current_indent_is_tab = false; } else { base_indent_is_tab = false; }
                    }
                },
                _ => {}
            }
        }
        if let Some(rule) = current { config.rules.push(rule); }
        config
    }

    fn style_for(&self, path: &Path) -> EditorStyle {
        let name = path.to_string_lossy().replace('\\', "/");
        let mut style = self.base;
        for rule in &self.rules {
            if rule.matcher.is_match(&name) { overlay(&mut style, rule.style); }
        }
        style
    }
}
