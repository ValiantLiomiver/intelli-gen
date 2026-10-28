use std::{fs, path::Path};

use globset::{GlobBuilder, GlobMatcher};

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
pub struct ProjectEditorConfig {
    base: EditorStyle,
    rules: Vec<Rule>,
}

impl ProjectEditorConfig {
    /// Read the workspace-root EditorConfig without blocking the UI thread.
    pub fn load(root: &Path) -> Self {
        let path = root.join(".editorconfig");
        let Ok(metadata) = fs::symlink_metadata(&path) else { return Self::default(); };
        if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 256 * 1024 {
            return Self::default();
        }
        let Ok(text) = fs::read_to_string(path) else { return Self::default(); };
        Self::parse(&text)
    }

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

    pub fn style_for(&self, root: &Path, file: &Path) -> EditorStyle {
        let Ok(relative) = file.strip_prefix(root) else { return self.base; };
        let path = relative.to_string_lossy().replace('\\', "/");
        let mut style = self.base;
        for rule in &self.rules {
            if rule.matcher.is_match(&path) {
                if rule.style.indent_size.is_some() { style.indent_size = rule.style.indent_size; }
                if rule.style.insert_spaces.is_some() { style.insert_spaces = rule.style.insert_spaces; }
            }
        }
        style
    }
}
