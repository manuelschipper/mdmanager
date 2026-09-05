use std::path::{Component, Path};

use serde::Deserialize;

/// Section schema shared by Global and Project; each scope resolves its own paths.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Section {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) path: String,
}

/// Format-1 composition preserves a single source byte-for-byte and orders multiple sources as supplied.
pub(crate) fn render_format_1(contents: &[&str]) -> String {
    match contents {
        [] => String::new(),
        [content] => (*content).to_owned(),
        contents => {
            contents
                .iter()
                .map(|content| content.trim_matches(['\r', '\n']))
                .collect::<Vec<_>>()
                .join("\n\n")
                + "\n"
        }
    }
}

/// Section and catalog ids start with a lowercase letter and contain lowercase letters, digits, hyphens or underscores.
pub(crate) fn validate_id(kind: &str, id: &str) -> Result<(), String> {
    let mut chars = id.chars();
    if !matches!(chars.next(), Some('a'..='z'))
        || !chars.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_')
        })
    {
        return Err(format!("invalid {kind} id {id}"));
    }
    Ok(())
}

/// Section relative paths contain only normal components; scopes own their base directory.
pub(crate) fn validate_relative_path(kind: &str, raw: &str) -> Result<(), String> {
    let path = Path::new(raw);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("{kind} path must be a clean relative path: {raw}"));
    }
    Ok(())
}
