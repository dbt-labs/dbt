use std::collections::HashMap;

use dbt_common::{FsResult, hashing::code_hash, io_args::IoArgs, stdfs};
use dbt_schemas::state::{DbtAsset, DbtState};

use crate::types::InputKind;

// Helper function to convert asset path to project-relative path
fn asset_to_relative_path(io: &IoArgs, asset: &DbtAsset) -> std::path::PathBuf {
    let absolute_path = asset.base_path.join(&asset.path);
    absolute_path
        .strip_prefix(&io.in_dir)
        .unwrap_or(&absolute_path)
        .to_owned()
}

// Store file path to InputKind mapping
#[derive(Debug, Clone, Default)]
pub struct CompleteStateWithKind {
    kinds: HashMap<String, InputKind>,
    session_files: Vec<String>,
}

impl CompleteStateWithKind {
    pub fn new() -> Self {
        Self {
            kinds: HashMap::new(),
            session_files: Vec::new(),
        }
    }

    pub fn from_dbt_state(arg: &IoArgs, dbt_state: &DbtState) -> FsResult<Self> {
        let mut registry = Self::new();

        // Register session-level config files
        registry.register_session_files(dbt_state);

        // Register package assets
        for package in &dbt_state.packages {
            registry.register_assets(arg, &package.macro_files, InputKind::Macro)?;
            registry.register_assets(arg, &package.model_sql_files, InputKind::Code)?;
            registry.register_assets(arg, &package.test_files, InputKind::Code)?;
            registry.register_assets(arg, &package.seed_files, InputKind::Seed)?;
            registry.register_assets(arg, &package.docs_files, InputKind::Doc)?;
            registry.register_assets(arg, &package.snapshot_files, InputKind::Code)?;
            registry.register_assets(arg, &package.analysis_files, InputKind::Code)?;
            registry.register_assets(arg, &package.dbt_properties, InputKind::Config)?;
        }
        Ok(registry)
    }

    /// Register well-known session-level config files
    /// We only register the root package's files.
    fn register_session_files(&mut self, dbt_state: &DbtState) {
        let root_package = dbt_state.root_package();
        if let Some(paths) = root_package
            .all_paths
            .get(&dbt_schemas::state::ResourcePathKind::SessionPaths)
        {
            for (path, _) in paths {
                let session_file = path.to_str().unwrap_or_default().to_string();
                self.kinds.insert(session_file.clone(), InputKind::Config);
                self.session_files.push(session_file);
            }
        }
    }

    fn register_assets(
        &mut self,
        io: &IoArgs,
        assets: &Vec<DbtAsset>,
        input_kind: InputKind,
    ) -> FsResult<()> {
        for asset in assets {
            let relative_path = asset_to_relative_path(io, asset);
            self.kinds
                .insert(relative_path.display().to_string(), input_kind);
        }
        Ok(())
    }

    pub fn keys(&self) -> Vec<String> {
        self.kinds.keys().cloned().collect()
    }

    pub fn get_kind(&self, path: &str) -> Option<InputKind> {
        self.kinds.get(path).copied()
    }

    pub fn get_session_files(&self) -> &[String] {
        &self.session_files
    }

    pub fn build_cas(&self, io: &IoArgs) -> FsResult<HashMap<String, String>> {
        let mut cas = HashMap::new();

        for key in self.keys() {
            let absolute_path = io.in_dir.join(&key);
            if absolute_path.exists() {
                let code = stdfs::read_to_string(&absolute_path)?;
                cas.insert(code_hash(&code), code);
            }
        }

        Ok(cas)
    }
}
