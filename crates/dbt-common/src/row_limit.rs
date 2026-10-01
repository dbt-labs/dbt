use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Wrapper type that handles row limiting with special -1 case for unlimited rows
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RowLimit(pub Option<u32>);

impl Default for RowLimit {
    fn default() -> Self {
        RowLimit(Some(10)) // Default to 10 rows
    }
}

impl RowLimit {
    /// Get the inner Option<u32> value
    pub fn value(&self) -> Option<u32> {
        self.0
    }

    /// Check if the value is None (was -1, meaning unlimited)
    pub fn is_unlimited(&self) -> bool {
        self.0.is_none()
    }

    /// Get the value or a default
    pub fn unwrap_or(&self, default: u32) -> u32 {
        self.0.unwrap_or(default)
    }
}

// Automatic conversion to Option<usize> via From trait
impl From<RowLimit> for Option<usize> {
    fn from(limit: RowLimit) -> Self {
        limit.0.map(|n| n as usize)
    }
}

impl From<&RowLimit> for Option<usize> {
    fn from(limit: &RowLimit) -> Self {
        limit.0.map(|n| n as usize)
    }
}

impl FromStr for RowLimit {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.parse::<i32>() {
            Ok(-1) => Ok(RowLimit(None)),
            Ok(n) if n >= 0 => Ok(RowLimit(Some(n as u32))),
            Ok(n) => Err(format!(
                "Row limit must be -1 (unlimited) or a non-negative number, got: {n}"
            )),
            Err(_) => Err(format!("Invalid number: {s}")),
        }
    }
}
