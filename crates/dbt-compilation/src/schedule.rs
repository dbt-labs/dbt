use std::collections::HashSet;
use std::sync::Arc;

use dbt_common::io_utils::YML_EXT;
use dbt_common::node_selector::IndirectSelection;
use dbt_common::path::DbtPath;
use dbt_schemas::state::CacheState;

#[derive(Debug)]
pub struct DbtCustomScheduleDescription {
    pub unique_ids: Vec<String>,
    pub include_parents: bool,
    pub include_children: bool,
    /// Indirect selection mode used when expanding the atoms built from
    /// `unique_ids`. This controls whether nodes that indirectly attach to the
    /// selected nodes (e.g. tests) are pulled into the schedule. Callers set this
    /// to reflect the semantics they want (e.g. `Empty` when the originating
    /// command does not execute tests).
    pub indirect_selection: IndirectSelection,
}

pub enum DbtScheduleDescription<'a> {
    Default,
    Custom(&'a DbtCustomScheduleDescription),
    CacheChanges(&'a DbtProjectCompilationCacheChanges),
}

/// Contains information about which nodes and files
/// have been created, deleted, changed, or unchanged
/// from initialization.
/// This is used to determine what to invalidate in a compilation cache
/// when running tasks.
#[derive(Default)]
pub struct DbtProjectCompilationCacheChanges {
    pub(crate) build_cache_changes: CacheState,
}

impl DbtProjectCompilationCacheChanges {
    pub fn new(build_cache_changes: CacheState) -> Self {
        Self {
            build_cache_changes,
        }
    }

    pub fn new_files(&self) -> &HashSet<DbtPath> {
        &self.build_cache_changes.file_changes.new_files
    }

    pub fn deleted_files(&self) -> &HashSet<DbtPath> {
        &self.build_cache_changes.file_changes.deleted_files
    }

    pub fn changed_files(&self) -> &HashSet<DbtPath> {
        &self.build_cache_changes.file_changes.changed_files
    }

    pub fn did_any_yml_files_change(&self) -> bool {
        self.build_cache_changes
            .file_changes
            .changed_files
            .iter()
            .any(|x| x.has_extension(YML_EXT))
    }

    pub fn as_cache_state(&self) -> &CacheState {
        &self.build_cache_changes
    }

    pub fn changed_nodes(&self) -> &Arc<HashSet<String>> {
        &self.build_cache_changes.changed_nodes
    }

    pub fn impacted_nodes(&self) -> &Arc<HashSet<String>> {
        &self.build_cache_changes.impacted_nodes
    }
}
