use std::{collections::HashSet, path::PathBuf};

use dbt_common::CompiledSpans;
use dbt_common::{
    FsResult, MacroSpan,
    constants::DBT_COMPILED_DIR_NAME,
    io_args::IoArgs,
    path::{get_snapshot_write_path, get_target_write_path},
    stdfs,
};
use dbt_frontend_common::span::ReclassifySpan;
use dbt_schemas::schemas::CommonAttributes;
use dbt_tasks_core::CompiledSqlCache;
use serde::{Deserialize, Serialize};

/// On-disk shape of the `*.macro_spans.json` sidecar. Stores the macro spans
/// alongside the reclassify offset records so a cache hit can restore both
/// without re-rendering. `reclassify_spans` is `#[serde(default)]` purely as
/// cheap insurance against a partial write.
#[derive(Serialize, Deserialize)]
pub struct CachedSpans {
    pub macro_spans: Vec<MacroSpan>,
    #[serde(default)]
    pub reclassify_spans: Vec<ReclassifySpan>,
}

#[derive(Default)]
pub struct CompiledSqlCacheImpl {
    valid_nodes: parking_lot::RwLock<HashSet<String>>,
}

impl CompiledSqlCache for CompiledSqlCacheImpl {
    fn get_compiled_sql_path(&self, io: &IoArgs, common: &CommonAttributes) -> PathBuf {
        // Snapshots always use the many-to-one nested path: original_file_path/name.sql.
        // A single .sql file can contain multiple snapshot blocks, so the basename
        // heuristic in get_target_write_path produces EISDIR when one snapshot's name
        // matches the source filename. Mirrors dbt-core SnapshotNode.get_target_write_path
        // (dbt-core#12693). We detect snapshots via the unique_id prefix because
        // CommonAttributes does not carry resource_type.
        if common.unique_id.starts_with("snapshot.") {
            return get_snapshot_write_path(
                &io.out_dir.join(DBT_COMPILED_DIR_NAME),
                &common.package_name,
                &common.original_file_path,
                &common.name,
            );
        }
        get_target_write_path(
            &io.in_dir,
            &io.out_dir.join(DBT_COMPILED_DIR_NAME),
            &common.package_name,
            &common.path,
            &common.original_file_path,
        )
    }

    fn try_get_compiled_sql(
        &self,
        io: &IoArgs,
        common: &CommonAttributes,
    ) -> Option<(String, Vec<MacroSpan>, Vec<ReclassifySpan>)> {
        {
            let valid_nodes = self.valid_nodes.read();
            if !valid_nodes.contains(&common.unique_id) {
                return None;
            }
        }

        let absolute_compiled_path = self.get_compiled_sql_path(io, common);
        let absolute_macro_span_path = absolute_compiled_path.with_extension("macro_spans.json");

        let Ok(rendered_sql_maybe_with_cte) = std::fs::read_to_string(absolute_compiled_path)
        else {
            return None;
        };
        let Ok(macro_spans_json) = std::fs::read_to_string(absolute_macro_span_path) else {
            return None;
        };
        let Ok(CachedSpans {
            macro_spans,
            reclassify_spans,
        }) = serde_json::from_str(&macro_spans_json)
        else {
            return None;
        };
        Some((rendered_sql_maybe_with_cte, macro_spans, reclassify_spans))
    }

    fn set_compiled_sql(
        &self,
        io: &IoArgs,
        common: &CommonAttributes,
        rendered_sql_maybe_with_cte: &str,
        spans: &dyn CompiledSpans,
    ) -> FsResult<()> {
        {
            let mut valid_nodes = self.valid_nodes.write();
            valid_nodes.insert(common.unique_id.clone());
        }

        let absolute_compiled_path = self.get_compiled_sql_path(io, common);
        let absolute_macro_span_path = absolute_compiled_path.with_extension("macro_spans.json");

        let cached_spans = CachedSpans {
            macro_spans: spans.macro_spans().to_vec(),
            reclassify_spans: spans.reclassify_spans().unwrap_or_default().to_vec(),
        };

        stdfs::create_dir_all(absolute_compiled_path.parent().unwrap())?;
        stdfs::write(&absolute_compiled_path, rendered_sql_maybe_with_cte)?;
        stdfs::write(
            absolute_macro_span_path,
            serde_json::to_string_pretty(&cached_spans).unwrap(),
        )?;
        Ok(())
    }

    fn clear(&self, unique_id: &str) {
        let mut valid_nodes = self.valid_nodes.write();
        valid_nodes.remove(unique_id);
    }
}
