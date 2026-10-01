use regex::Regex;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use crate::{ErrorCode, FsError, FsResult};

/// Extracts a substring enclosed in the given regex pattern
pub fn find_enclosed_substring(msg: &str, re: &Regex) -> Option<String> {
    if let Some(captures) = re.captures(msg)
        && let Some(substring) = captures.get(1)
    {
        return Some(substring.as_str().to_string());
    }
    None
}

/// Finds line, column, and index positions of a token in a file
pub fn find_locations(token: &str, file: &Path) -> FsResult<Option<(u32, u32, u32)>> {
    let output = None;
    // the token might be a dotted name, first try to find the full name, then the last part
    let tokens = token.split('.').collect::<Vec<&str>>();

    if tokens.is_empty() {
        return Ok(output);
    }

    // Try to find positions for each token part
    let positions = find_token_positions_in_file(file, token)?;
    if !positions.is_empty() {
        return Ok(Some(positions[0]));
    }

    // If full token not found, try the last part
    if tokens.len() > 1 {
        let last_token = tokens[tokens.len() - 1];
        let positions = find_token_positions_in_file(file, last_token)?;
        if !positions.is_empty() {
            return Ok(Some(positions[0]));
        }
    }

    Ok(output)
}

fn find_token_positions_in_file(file: &Path, token: &str) -> FsResult<Vec<(u32, u32, u32)>> {
    let file = std::fs::File::open(file).map_err(|e| {
        FsError::new(
            ErrorCode::IoError,
            format!("Failed to open file {}: {}", file.display(), e),
        )
    })?;
    let reader = BufReader::new(file);

    let mut positions = Vec::new();
    let mut pos: u32 = 0;
    for (line_number, line) in reader.lines().enumerate() {
        let line = line
            .map_err(|e| FsError::new(ErrorCode::IoError, format!("Failed to read line: {e}")))?;
        let column_number: u32 = 0;
        let remaining_line = &line[..];

        if let Some(index) = remaining_line.find(token) {
            let start_column = column_number + index as u32;
            positions.push((line_number as u32 + 1, start_column + 1, pos + index as u32));
        }
        pos += line.len() as u32 + 1; // TODO: 1 means the newline character, maybe \r\n
    }

    Ok(positions)
}

/// Check if SDF debug mode is enabled
pub fn is_sdf_debug() -> bool {
    std::env::var("SDF_DEBUG")
        .as_ref()
        .map(String::as_str)
        .map(str::parse::<i32>)
        .map(Result::unwrap_or_default)
        .unwrap_or_default()
        > 0
}

/// Wrapper around [`std::fs::canonicalize`] that returns a useful error in case of failure.
/// This is the same as dbt_common::stdfs::canonicalize inlined in this crate
/// to avoid a dependency on dbt_common.
pub fn canonicalize<P: AsRef<Path>>(path: P) -> Result<PathBuf, std::io::Error> {
    let path = path.as_ref();
    #[cfg(not(target_os = "windows"))]
    {
        // Only place in our codebase where std::fs::canonicalize is allowed:
        #[allow(clippy::disallowed_methods)]
        std::fs::canonicalize(path)
    }
    #[cfg(target_os = "windows")]
    {
        dunce::canonicalize(path)
    }
}
