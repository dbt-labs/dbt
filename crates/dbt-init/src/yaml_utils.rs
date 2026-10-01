use crate::{ErrorCode, FsResult, fs_err};
use std::fs;
use std::path::Path;

/// Remove a top-level mapping entry from a YAML file by key while preserving formatting and comments.
///
/// This is a best-effort, indentation-aware text manipulation. It assumes the target key starts
/// at indentation level 0 as `<key>:` and removes all lines until the next line with indentation
/// level 0 or EOF.
pub fn remove_top_level_key(path: &Path, key: &str) -> FsResult<String> {
    let content = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };
    Ok(remove_top_level_key_from_str(content, key))
}

/// Append a YAML snippet at the end of a file with a preceding newline if needed, preserving
/// the existing content as-is.
pub fn append_yaml_block(path: &Path, block: &str) -> FsResult<()> {
    let mut content = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };

    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(block);
    if !content.ends_with('\n') {
        content.push('\n');
    }
    fs::write(path, content)?;
    Ok(())
}

/// Utility used by `remove_top_level_key` to operate on in-memory strings.
pub fn remove_top_level_key_from_str(content: String, key: &str) -> String {
    if content.is_empty() {
        return content;
    }
    let mut new_content = String::new();
    let mut skipping = false;
    let mut target_indent = 0usize;

    for line in content.lines() {
        let trimmed = line.trim_start();
        let current_indent = line.len() - trimmed.len();

        if !skipping {
            if trimmed.starts_with(key)
                && trimmed[key.len()..].starts_with(':')
                && current_indent == 0
            {
                // Start skipping this block
                skipping = true;
                target_indent = current_indent;
                continue;
            }
            new_content.push_str(line);
            new_content.push('\n');
        } else if !trimmed.is_empty() {
            if current_indent <= target_indent {
                // Stop skipping and write this top-level line
                skipping = false;
                new_content.push_str(line);
                new_content.push('\n');
            } else {
                continue;
            }
        } else {
            // ignore blank lines while skipping
            continue;
        }
    }

    new_content
}

/// Check if a top-level key exists in the YAML text.
pub fn has_top_level_key(content: &str, key: &str) -> bool {
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with(key) && trimmed[key.len()..].starts_with(':') {
            let indent = line.len() - trimmed.len();
            if indent == 0 {
                return true;
            }
        }
    }
    false
}

/// Read a YAML file with serde when you only need data access.
pub fn read_yaml_as_value<T: serde::de::DeserializeOwned>(path: &Path) -> FsResult<Option<T>> {
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)?;
    match dbt_yaml::from_str::<T>(&content) {
        Ok(v) => Ok(Some(v)),
        Err(e) => Err(fs_err!(
            ErrorCode::IoError,
            "Failed to parse YAML at {}: {}",
            path.display(),
            e
        )),
    }
}

/// Parse YAML content and check if a top-level key exists using serde.
pub fn has_top_level_key_parsed_str(content: &str, key: &str) -> bool {
    if let Ok(val) = dbt_yaml::from_str::<dbt_yaml::Value>(content)
        && let Some(mapping) = val.as_mapping()
    {
        return mapping.iter().any(|(k, _)| k.as_str() == Some(key));
    }
    false
}

/// Read and parse YAML file with serde and check if a top-level key exists.
pub fn has_top_level_key_parsed_file(path: &Path, key: &str) -> FsResult<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let content = fs::read_to_string(path)?;
    Ok(has_top_level_key_parsed_str(&content, key))
}

/// Return all top-level mapping key names from a YAML string, preserving document order.
///
/// Uses the same text-scan approach as `has_top_level_key` so that it is tolerant of
/// non-standard formatting and avoids a full parse round-trip.
pub fn list_top_level_keys(content: &str) -> Vec<String> {
    let mut keys = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        // Only consider zero-indent, non-blank, non-comment lines.
        if indent != 0 || trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(colon_pos) = trimmed.find(':') {
            let key = trimmed[..colon_pos].trim();
            // Reject keys that contain spaces (those would be values, not mapping keys).
            if !key.is_empty() && !key.contains(' ') {
                keys.push(key.to_string());
            }
        }
    }
    keys
}

/// Return all top-level mapping key names from a YAML file.
/// Returns an empty vec if the file does not exist.
pub fn list_top_level_keys_from_file(path: &Path) -> FsResult<Vec<String>> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let content = fs::read_to_string(path)?;
    Ok(list_top_level_keys(&content))
}
