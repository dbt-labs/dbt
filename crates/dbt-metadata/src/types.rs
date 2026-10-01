use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    #[default]
    Code,
    Config,
    Macro,
    Seed,
    Doc,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct InputFile {
    pub path: String,
    pub file_cas_hash: String,
    pub input_kind: InputKind,
}
