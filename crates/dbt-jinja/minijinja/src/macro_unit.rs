//! A module for macro units.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{machinery::Span, ArgSpec};

/// A unit of a macro.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MacroUnit {
    /// The info of the macro.
    pub info: MacroInfo,
    /// The SQL of the macro.
    pub sql: String,
}

/// The info of the macro.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MacroInfo {
    /// The name of the macro.
    pub name: String,
    /// The relative path of the macro.
    pub path: PathBuf,
    /// The start span of the macro.
    pub span: Span,
    /// The funcsign of the macro.
    pub funcsign: Option<String>,
    /// The args of the macro.
    pub args: Vec<ArgSpec>,
    /// The unique id of the macro.
    pub unique_id: String,
    /// The start span of the macro name.
    pub name_span: Span,
}
