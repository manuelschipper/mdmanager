//! Claude settings exclusions preserve unrelated JSON text; Local owns writes and recovery state.
use serde_json::{Map, Value};
use std::path::Path;

/// Claude exclusion lookup validates the settings object without rewriting it.
pub(super) fn contains_exclusion(source: &str, path: &Path, value: &str) -> Result<bool, String> {
    Ok(parse_json_object(source, path)?
        .get("claudeMdExcludes")
        .and_then(Value::as_array)
        .is_some_and(|entries| entries.iter().any(|entry| entry.as_str() == Some(value))))
}

/// Claude exclusion insertion refuses duplicate ownership and preserves surrounding settings text.
pub(super) fn insert_exclusion(source: &str, path: &Path, value: &str) -> Result<String, String> {
    if contains_exclusion(source, path, value)? {
        return Err("the selected Claude source is already excluded".into());
    }
    insert_json_array_string(source, "claudeMdExcludes", value)
}

/// Claude exclusion removal removes only the owned entry, retaining unrelated settings.
pub(super) fn remove_exclusion(source: &str, value: &str) -> Result<String, String> {
    remove_json_array_string(source, "claudeMdExcludes", value)
}

/// Claude recovery requires an original settings object before Local writes any files.
pub(super) fn parse_json_object(source: &str, path: &Path) -> Result<Map<String, Value>, String> {
    let value: Value = serde_json::from_str(source)
        .map_err(|error| format!("invalid {}: {error}", path.display()))?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| format!("{} must contain a JSON object", path.display()))
}

fn insert_json_array_string(source: &str, key: &str, value: &str) -> Result<String, String> {
    let value = serde_json::to_string(value)
        .map_err(|error| format!("cannot serialize Claude exclusion: {error}"))?;
    if let Some((open, close)) = json_array_range(source, key)? {
        let interior = &source[open + 1..close];
        let insertion = if interior.trim().is_empty() {
            value
        } else if interior.contains('\n') {
            let indent = interior
                .trim_end()
                .rsplit('\n')
                .next()
                .unwrap_or_default()
                .chars()
                .take_while(|character| character.is_whitespace())
                .collect::<String>();
            format!(",\n{indent}{value}")
        } else {
            format!(", {value}")
        };
        let insert_at = close - interior.len() + interior.trim_end().len();
        let mut output = source.to_owned();
        output.insert_str(insert_at, &insertion);
        return Ok(output);
    }

    let (open, close) = top_level_object_range(source)?;
    let interior = &source[open + 1..close];
    let property = format!("\"{key}\": [{value}]");
    let insertion = if interior.trim().is_empty() {
        format!("\n  {property}\n")
    } else if interior.contains('\n') {
        let indent = interior
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or_default()
            .chars()
            .take_while(|character| character.is_whitespace())
            .collect::<String>();
        format!(",\n{indent}{property}")
    } else {
        format!(", {property}")
    };
    let insert_at = open + 1 + interior.trim_end().len();
    let mut output = source.to_owned();
    output.insert_str(insert_at, &insertion);
    Ok(output)
}

fn remove_json_array_string(source: &str, key: &str, value: &str) -> Result<String, String> {
    parse_json_object(source, Path::new("Claude settings"))?;
    let (open, close) = json_array_range(source, key)?
        .ok_or_else(|| format!("Claude settings no longer contain {key}"))?;
    let needle = serde_json::to_string(value)
        .map_err(|error| format!("cannot serialize Claude exclusion: {error}"))?;
    for (relative, _) in source[open + 1..close].match_indices(&needle) {
        let start = open + 1 + relative;
        let end = start + needle.len();
        let before = source[open..start].trim_end();
        let after = source[end..=close].trim_start();
        if !matches!(before.as_bytes().last(), Some(b'[' | b','))
            || !matches!(after.as_bytes().first(), Some(b',' | b']'))
        {
            continue;
        }

        let mut output = source.to_owned();
        if after.starts_with(',') {
            let comma = end + source[end..=close].len() - after.len();
            let leading_whitespace = open + before.len();
            output.replace_range(leading_whitespace..comma + 1, "");
        } else if before.ends_with(',') {
            let comma = open + before.len() - 1;
            output.replace_range(comma..end, "");
        } else {
            output.replace_range(start..end, "");
        }
        return Ok(output);
    }
    Err(format!(
        "Claude settings no longer contain the mdmanager.ai-owned exclusion {value}"
    ))
}

fn json_array_range(source: &str, key: &str) -> Result<Option<(usize, usize)>, String> {
    let bytes = source.as_bytes();
    let mut index = 0;
    let mut object_depth = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                let end = json_string_end(bytes, index)?;
                if object_depth == 1 {
                    let decoded: String = serde_json::from_str(&source[index..end])
                        .map_err(|error| format!("invalid JSON property: {error}"))?;
                    let mut next = skip_json_space(bytes, end);
                    if decoded == key && bytes.get(next) == Some(&b':') {
                        next = skip_json_space(bytes, next + 1);
                        if bytes.get(next) != Some(&b'[') {
                            return Err(format!("{key} is not an array"));
                        }
                        return Ok(Some((next, matching_json_bracket(bytes, next)?)));
                    }
                }
                index = end;
                continue;
            }
            b'{' => object_depth += 1,
            b'}' => object_depth = object_depth.saturating_sub(1),
            _ => {}
        }
        index += 1;
    }
    Ok(None)
}

fn top_level_object_range(source: &str) -> Result<(usize, usize), String> {
    let bytes = source.as_bytes();
    let mut index = skip_json_space(bytes, 0);
    if bytes.get(index) != Some(&b'{') {
        return Err("Claude settings must contain a JSON object".into());
    }
    let open = index;
    index += 1;
    let mut depth = 1usize;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                index = json_string_end(bytes, index)?;
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok((open, index));
                }
            }
            _ => {}
        }
        index += 1;
    }
    Err("Claude settings JSON object is not closed".into())
}

fn matching_json_bracket(bytes: &[u8], open: usize) -> Result<usize, String> {
    let mut index = open + 1;
    let mut depth = 1usize;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                index = json_string_end(bytes, index)?;
                continue;
            }
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    Err("claudeMdExcludes array is not closed".into())
}

fn json_string_end(bytes: &[u8], start: usize) -> Result<usize, String> {
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => return Ok(index + 1),
            _ => index += 1,
        }
    }
    Err("unterminated JSON string".into())
}

fn skip_json_space(bytes: &[u8], mut index: usize) -> usize {
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inserts_into_an_existing_exclusion_array_without_reformatting() {
        let source = "{\n  \"before\": 1,\n  \"claudeMdExcludes\": [\n    \"/before.md\"\n  ],\n  \"after\": 2\n}\n";
        let updated = insert_exclusion(source, Path::new("Claude settings"), "/new.md").unwrap();
        assert_eq!(
            updated,
            "{\n  \"before\": 1,\n  \"claudeMdExcludes\": [\n    \"/before.md\",\n    \"/new.md\"\n  ],\n  \"after\": 2\n}\n"
        );
    }

    #[test]
    fn removes_only_the_owned_exclusion_without_reformatting() {
        let source = "{\n  \"before\": 1,\n  \"claudeMdExcludes\": [\n    \"/before.md\",\n    \"/owned.md\",\n    \"/after.md\"\n  ],\n  \"after\": 2\n}\n";
        let updated = remove_exclusion(source, "/owned.md").unwrap();
        assert_eq!(
            updated,
            "{\n  \"before\": 1,\n  \"claudeMdExcludes\": [\n    \"/before.md\",\n    \"/after.md\"\n  ],\n  \"after\": 2\n}\n"
        );
    }
}
