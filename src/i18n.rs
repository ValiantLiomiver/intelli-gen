use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use eframe::egui;
use serde::{Deserialize, Serialize};

const ENGLISH: &str = include_str!("../resources/i18n/en.ini");
const ITALIAN: &str = include_str!("../resources/i18n/it.ini");

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LanguagePreference {
    #[default]
    System,
    English,
    Italian,
}

impl LanguagePreference {
    fn resolved(self) -> ResolvedLanguage {
        match self {
            Self::English => ResolvedLanguage::English,
            Self::Italian => ResolvedLanguage::Italian,
            Self::System => {
                let language = sys_locale::get_locale()
                    .and_then(|locale| locale.split(['-', '_']).next().map(str::to_owned));
                if language.is_some_and(|language| language.eq_ignore_ascii_case("it")) {
                    ResolvedLanguage::Italian
                } else {
                    ResolvedLanguage::English
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResolvedLanguage {
    English,
    Italian,
}

impl ResolvedLanguage {
    fn file_name(self) -> &'static str {
        match self {
            Self::English => "en.ini",
            Self::Italian => "it.ini",
        }
    }
}

#[derive(Clone)]
pub struct Translator {
    catalog: Arc<HashMap<String, String>>,
    english: Arc<HashMap<String, String>>,
}

fn translator_id() -> egui::Id {
    egui::Id::new("intelli_gen_translator")
}

pub fn install(ctx: &egui::Context, translator: &Translator) {
    ctx.data_mut(|data| data.insert_temp(translator_id(), translator.clone()));
}

pub fn text(ctx: &egui::Context, key: &str) -> String {
    ctx.data(|data| data.get_temp::<Translator>(translator_id()))
        .map(|translator| translator.text(key).into_owned())
        .unwrap_or_else(|| key.to_owned())
}

pub fn ui_text(ui: &egui::Ui, key: &str) -> String {
    text(ui.ctx(), key)
}

pub fn ui_format(ui: &egui::Ui, key: &str, values: &[(&str, &str)]) -> String {
    format(ui.ctx(), key, values)
}

pub fn format(ctx: &egui::Context, key: &str, values: &[(&str, &str)]) -> String {
    ctx.data(|data| data.get_temp::<Translator>(translator_id()))
        .map(|translator| translator.format(key, values))
        .unwrap_or_else(|| {
            let mut output = key.to_owned();
            for (name, value) in values {
                output = output.replace(&format!("{{{name}}}"), value);
            }
            output
        })
}

pub fn message(ctx: &egui::Context, source: &str) -> String {
    ctx.data(|data| data.get_temp::<Translator>(translator_id()))
        .map(|translator| translator.message(source))
        .unwrap_or_else(|| source.to_owned())
}

impl Translator {
    pub fn load(
        preference: LanguagePreference,
        config_dir: Option<&Path>,
    ) -> (Self, Option<String>) {
        let language = preference.resolved();
        let mut english = parse_catalog(ENGLISH).unwrap_or_default();
        let mut italian = parse_catalog(ITALIAN).unwrap_or_default();
        let mut error = None;

        if let Some(directory) = config_dir {
            let override_dir = directory.join("languages");
            for (catalog_language, catalog) in [
                (ResolvedLanguage::English, &mut english),
                (ResolvedLanguage::Italian, &mut italian),
            ] {
                let path = override_dir.join(catalog_language.file_name());
                match fs::read_to_string(&path) {
                    Ok(contents) => match parse_catalog(&contents) {
                        Ok(overrides) => catalog.extend(overrides),
                        Err(message) => {
                            error.get_or_insert_with(|| format!("{}: {message}", path.display()));
                        }
                    },
                    Err(read_error) if read_error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(read_error) => {
                        error.get_or_insert_with(|| format!("{}: {read_error}", path.display()));
                    }
                }
            }
        }

        let mut selected = match language {
            ResolvedLanguage::English => english.clone(),
            ResolvedLanguage::Italian => {
                let mut selected = italian;
                for (key, value) in &english {
                    selected.entry(key.clone()).or_insert_with(|| value.clone());
                }
                selected
            }
        };
        // English is the final fallback for any missing Italian entries.
        for (key, value) in &english {
            selected.entry(key.clone()).or_insert_with(|| value.clone());
        }

        (
            Self {
                catalog: Arc::new(selected),
                english: Arc::new(english),
            },
            error,
        )
    }

    pub fn text<'a>(&'a self, key: &'a str) -> Cow<'a, str> {
        self.catalog
            .get(key)
            .or_else(|| self.english.get(key))
            .map(|value| Cow::Borrowed(value.as_str()))
            .unwrap_or(Cow::Borrowed(key))
    }

    pub fn format(&self, key: &str, values: &[(&str, &str)]) -> String {
        let mut output = self.text(key).into_owned();
        for (name, value) in values {
            output = output.replace(&format!("{{{name}}}"), value);
        }
        output
    }

    pub fn message(&self, source: &str) -> String {
        if self.catalog.contains_key(source) || self.english.contains_key(source) {
            return self.text(source).into_owned();
        }
        for (template, _) in self.catalog.iter().filter(|(key, _)| key.contains('{')) {
            let Some(parameters) = match_template(template, source) else {
                continue;
            };
            let references: Vec<_> = parameters
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();
            return self.format(template, &references);
        }
        source.to_owned()
    }
}

fn match_template(template: &str, source: &str) -> Option<Vec<(String, String)>> {
    let mut template_offset = 0;
    let mut source_offset = 0;
    let mut parameters = Vec::new();
    loop {
        let relative_open = template[template_offset..].find('{');
        let Some(open) = relative_open.map(|offset| template_offset + offset) else {
            let suffix = &template[template_offset..];
            return (source[source_offset..] == *suffix).then_some(parameters);
        };
        let prefix = &template[template_offset..open];
        if !source[source_offset..].starts_with(prefix) {
            return None;
        }
        source_offset += prefix.len();
        let close = template[open + 1..].find('}')? + open + 1;
        let name = &template[open + 1..close];
        if name.is_empty() {
            return None;
        }
        template_offset = close + 1;
        let next_open = template[template_offset..]
            .find('{')
            .map(|offset| template_offset + offset);
        let next_static_end = next_open.unwrap_or(template.len());
        let next_static = &template[template_offset..next_static_end];
        let value_end = if next_static.is_empty() {
            source.len()
        } else {
            source[source_offset..].find(next_static)? + source_offset
        };
        parameters.push((name.to_owned(), source[source_offset..value_end].to_owned()));
        source_offset = value_end;
        template_offset = next_static_end;
        if next_open.is_none() {
            if !source[source_offset..].starts_with(next_static) {
                return None;
            }
            source_offset += next_static.len();
            return (source_offset == source.len()).then_some(parameters);
        }
    }
}

fn parse_catalog(text: &str) -> Result<HashMap<String, String>, String> {
    let mut catalog = HashMap::new();
    for (index, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((key, value)) = split_entry(line) else {
            return Err(format!("invalid entry on line {}", index + 1));
        };
        let key = unescape(key.trim());
        if key.is_empty() {
            return Err(format!("empty key on line {}", index + 1));
        }
        if catalog
            .insert(key.clone(), unescape(value.trim()))
            .is_some()
        {
            return Err(format!("duplicate key `{key}` on line {}", index + 1));
        }
    }
    Ok(catalog)
}

fn split_entry(line: &str) -> Option<(&str, &str)> {
    let mut escaped = false;
    for (index, character) in line.char_indices() {
        if character == '=' && !escaped {
            return Some((&line[..index], &line[index + 1..]));
        }
        if character == '\\' {
            escaped = !escaped;
        } else {
            escaped = false;
        }
    }
    None
}

fn unescape(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(character) = chars.next() {
        if character == '\\' {
            match chars.next() {
                Some('n') => output.push('\n'),
                Some('r') => output.push('\r'),
                Some('=') => output.push('='),
                Some('\\') => output.push('\\'),
                Some(other) => {
                    output.push('\\');
                    output.push(other);
                }
                None => output.push('\\'),
            }
        } else {
            output.push(character);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_catalogs_parse_and_preserve_translation_parameters() {
        let english = parse_catalog(ENGLISH).expect("valid English catalog");
        let italian = parse_catalog(ITALIAN).expect("valid Italian catalog");
        assert_eq!(
            english.keys().collect::<std::collections::BTreeSet<_>>(),
            italian.keys().collect::<std::collections::BTreeSet<_>>(),
            "built-in catalogs must expose the same keys"
        );
        let parameters = |text: &str| {
            text.match_indices('{')
                .filter_map(|(start, _)| {
                    text[start + 1..]
                        .find('}')
                        .map(|end| text[start + 1..start + 1 + end].to_owned())
                })
                .collect::<std::collections::BTreeSet<_>>()
        };
        for (key, value) in &english {
            let translated = italian
                .get(key)
                .unwrap_or_else(|| panic!("missing Italian key: {key}"));
            assert_eq!(
                parameters(key),
                parameters(value),
                "English parameters: {key}"
            );
            assert_eq!(
                parameters(key),
                parameters(translated),
                "Italian parameters: {key}"
            );
        }
    }

    #[test]
    fn ini_catalog_parses_escaped_equals_and_newlines() {
        let catalog =
            parse_catalog("# comment\nEnvironment (NAME\\=value)=A\\=B\\nnext line\n; comment\n")
                .unwrap();

        assert_eq!(
            catalog.get("Environment (NAME=value)").map(String::as_str),
            Some("A=B\nnext line")
        );
    }

    #[test]
    fn ini_catalog_rejects_duplicate_and_invalid_entries() {
        assert!(parse_catalog("label=one\nlabel=two").is_err());
        assert!(parse_catalog("=empty key").is_err());
        assert!(parse_catalog("missing separator").is_err());
    }

    #[test]
    fn message_translates_templates_and_preserves_dynamic_paths() {
        let source = "Cannot save {path}: The file changed on disk.";
        let translated = "Impossibile salvare {path}: il file è cambiato su disco.";
        let translator = Translator {
            catalog: Arc::new(HashMap::from([(source.to_owned(), translated.to_owned())])),
            english: Arc::new(HashMap::from([(source.to_owned(), source.to_owned())])),
        };

        assert_eq!(
            translator.message(
                "Cannot save /tmp/project with spaces/Main.java: The file changed on disk."
            ),
            "Impossibile salvare /tmp/project with spaces/Main.java: il file è cambiato su disco."
        );
    }
}
