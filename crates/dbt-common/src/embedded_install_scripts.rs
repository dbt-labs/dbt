mod assets {
    #![allow(clippy::disallowed_methods)] // RustEmbed generates calls to std::path::Path::canonicalize

    use rust_embed::{EmbeddedFile, RustEmbed};

    #[derive(RustEmbed)]
    #[folder = "assets/"]
    pub struct Asset;

    impl Asset {
        pub fn load_file(filename: &str) -> Option<EmbeddedFile> {
            Self::get(filename)
        }
    }
}

use crate::FsResult;

/// Loads a script file from the embedded assets
pub fn load_script(filename: &str) -> FsResult<String> {
    match assets::Asset::load_file(filename) {
        Some(content) => Ok(String::from_utf8(content.data.into_owned())
            .unwrap_or_else(|_| panic!("{filename}:: corrupted asset: non UTF-8"))),
        None => panic!("{filename}:: missing asset"),
    }
}
