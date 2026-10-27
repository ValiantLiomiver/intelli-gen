use std::fs;
use std::path::Path;

use super::discovery::{descriptor_exists, normalize};
use super::{ProjectError, ProjectModule};

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Identifier(String),
    Literal(String),
    Symbol(char),
    Newline,
}

/// Parse literal settings includes without evaluating Groovy or Kotlin scripts.
pub fn modules(root: &Path) -> Result<Vec<ProjectModule>, ProjectError> {
    let groovy = root.join("settings.gradle");
    let kotlin = root.join("settings.gradle.kts");
    let settings = if descriptor_exists(&groovy)? {
        groovy
    } else if descriptor_exists(&kotlin)? {
        kotlin
    } else {
        return Ok(Vec::new());
    };
    let source = fs::read_to_string(&settings).map_err(|source| ProjectError {
        path: settings.clone(),
        source,
    })?;
    let tokens = tokenize(&source, &settings)?;
    let mut result: Vec<ProjectModule> = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        if tokens[index] != Token::Identifier("include".into()) {
            index += 1;
            continue;
        }
        index += 1;
        let parenthesized = tokens.get(index) == Some(&Token::Symbol('('));
        if parenthesized {
            index += 1;
        }
        let mut found = false;
        let mut expect_literal = true;
        loop {
            while parenthesized && tokens.get(index) == Some(&Token::Newline) {
                index += 1;
            }
            match tokens.get(index) {
                Some(Token::Literal(value)) if expect_literal => {
                    add_module(root, value, &settings, &mut result)?;
                    found = true;
                    expect_literal = false;
                    index += 1;
                }
                Some(Token::Symbol(',')) if !expect_literal => {
                    expect_literal = true;
                    index += 1;
                    while tokens.get(index) == Some(&Token::Newline) {
                        index += 1;
                    }
                }
                Some(Token::Symbol(')')) if parenthesized && found => {
                    index += 1;
                    break;
                }
                Some(Token::Newline | Token::Symbol(';') | Token::Symbol('}')) | None
                    if !parenthesized && found && !expect_literal =>
                {
                    break;
                }
                _ => {
                    return Err(ProjectError::invalid(
                        &settings,
                        "Gradle includes must use literal strings: include(\":app\", \":lib\")",
                    ));
                }
            }
        }
    }
    apply_directory_overrides(root, &tokens, &settings, &mut result)?;
    Ok(result)
}

fn apply_directory_overrides(
    root: &Path,
    source_tokens: &[Token],
    settings: &Path,
    modules: &mut [ProjectModule],
) -> Result<(), ProjectError> {
    let positions: Vec<_> = source_tokens
        .iter()
        .enumerate()
        .filter_map(|(index, token)| (*token != Token::Newline).then_some(index))
        .collect();
    let tokens: Vec<_> = positions
        .iter()
        .map(|index| source_tokens[*index].clone())
        .collect();
    for (index, token) in tokens.iter().enumerate() {
        if *token != Token::Identifier("projectDir".into())
            || tokens.get(index + 1) != Some(&Token::Symbol('='))
        {
            continue;
        }
        // Common Groovy/Kotlin form: project(":app").projectDir = file("custom/app").
        let Some(start) = index.checked_sub(5) else {
            return Err(ProjectError::invalid(
                settings,
                "Unsupported Gradle projectDir assignment",
            ));
        };
        let prefix = &tokens[start..index];
        let suffix = &tokens[index + 1..];
        let (name, directory) = match (prefix, suffix) {
            (
                [
                    Token::Identifier(project),
                    Token::Symbol('('),
                    Token::Literal(name),
                    Token::Symbol(')'),
                    Token::Symbol('.'),
                ],
                [
                    Token::Symbol('='),
                    Token::Identifier(file),
                    Token::Symbol('('),
                    Token::Literal(directory),
                    Token::Symbol(')'),
                    ..,
                ],
            ) if project == "project" && file == "file" && !directory.contains('$') => {
                (name, directory)
            }
            _ => {
                return Err(ProjectError::invalid(
                    settings,
                    "Gradle module directories require project(\":app\").projectDir = file(\"path\") with literal paths",
                ));
            }
        };
        // The matched `)` is at index + 5. Preserve source newlines so a
        // following statement is accepted, but a chained path expression cannot
        // silently be shortened to its initial file("...") call.
        if !assignment_ends(&source_tokens[positions[index + 5] + 1..]) {
            return Err(ProjectError::invalid(
                settings,
                "Unsupported expression after Gradle projectDir file(...); use a complete literal directory assignment",
            ));
        }
        let name = format!(":{}", name.trim_start_matches(':'));
        let module = modules
            .iter_mut()
            .find(|module| module.name == name)
            .ok_or_else(|| {
                ProjectError::invalid(
                    settings,
                    format!("projectDir refers to an unincluded module: {name}"),
                )
            })?;
        module.root = normalize(&root.join(directory));
    }
    Ok(())
}

fn assignment_ends(tokens: &[Token]) -> bool {
    match tokens.first() {
        None | Some(Token::Symbol(';' | '}')) => true,
        Some(Token::Newline) => match tokens.iter().find(|token| **token != Token::Newline) {
            // A chain/operator may continue an expression on the next line.
            Some(Token::Symbol(symbol)) => matches!(*symbol, ';' | '}'),
            Some(Token::Identifier(identifier)) => {
                !matches!(identifier.as_str(), "as" | "is" | "in" | "instanceof")
            }
            None => true,
            _ => false,
        },
        _ => false,
    }
}

fn add_module(
    root: &Path,
    value: &str,
    settings: &Path,
    result: &mut Vec<ProjectModule>,
) -> Result<(), ProjectError> {
    let value = value.trim_start_matches(':');
    if value.is_empty()
        || value.contains('$')
        || value.split(':').any(|part| {
            part.is_empty() || part == "." || part == ".." || part.contains(['/', '\\'])
        })
    {
        return Err(ProjectError::invalid(
            settings,
            "Gradle module paths must be literal colon-separated project names",
        ));
    }
    // Gradle creates parent projects implicitly for nested includes.
    let mut name = String::new();
    let mut module_root = root.to_path_buf();
    for component in value.split(':') {
        name.push(':');
        name.push_str(component);
        module_root.push(component);
        if !result.iter().any(|module| module.name == name) {
            result.push(ProjectModule {
                name: name.clone(),
                root: normalize(&module_root),
            });
        }
    }
    Ok(())
}

fn tokenize(source: &str, path: &Path) -> Result<Vec<Token>, ProjectError> {
    let mut chars = source.chars().peekable();
    let mut tokens = Vec::new();
    while let Some(character) = chars.next() {
        match character {
            '\n' => tokens.push(Token::Newline),
            character if character.is_whitespace() => {}
            '/' if chars.peek() == Some(&'/') => {
                chars.next();
                for character in chars.by_ref() {
                    if character == '\n' {
                        tokens.push(Token::Newline);
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut depth = 1usize;
                while let Some(character) = chars.next() {
                    if character == '/' && chars.peek() == Some(&'*') {
                        chars.next();
                        depth += 1;
                    } else if character == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    } else if character == '\n' {
                        tokens.push(Token::Newline);
                    }
                }
                if depth != 0 {
                    return Err(ProjectError::invalid(
                        path,
                        "Unterminated Gradle block comment",
                    ));
                }
            }
            quote @ ('\'' | '"') => {
                let mut value = String::new();
                let mut closed = false;
                while let Some(character) = chars.next() {
                    if character == quote {
                        closed = true;
                        break;
                    }
                    if character == '\\' {
                        if let Some(escaped) = chars.next() {
                            value.push(escaped);
                        }
                    } else {
                        value.push(character);
                    }
                }
                if !closed {
                    return Err(ProjectError::invalid(path, "Unterminated Gradle string"));
                }
                tokens.push(Token::Literal(value));
            }
            character if character.is_alphabetic() || character == '_' => {
                let mut identifier = String::from(character);
                while chars
                    .peek()
                    .is_some_and(|next| next.is_alphanumeric() || *next == '_')
                {
                    identifier.push(chars.next().unwrap());
                }
                tokens.push(Token::Identifier(identifier));
            }
            character => tokens.push(Token::Symbol(character)),
        }
    }
    Ok(tokens)
}
