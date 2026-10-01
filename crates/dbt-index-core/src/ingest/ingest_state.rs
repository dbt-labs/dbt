use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::epoch_layers;

// ---------------------------------------------------------------------------
// Directory layout under target/private/metadata/
// ---------------------------------------------------------------------------

pub const PARSE_NODES_SUBDIR: &str = "parse/nodes";
pub const PARSE_COLUMNS_SUBDIR: &str = "parse/columns";
pub const PARSE_ALIVE: &str = "parse/alive.parquet";
pub const PARSE_PROJECT: &str = "parse/project.parquet";
pub const PARSE_RESOLVER_STATE: &str = "parse/resolver_state.parquet";
/// Written only on cold start. Its mtime signals the last full parse; compile rows
/// with an older `ingested_at` are stale and should be ignored.
pub const PARSE_GENERATION: &str = "parse/generation.parquet";
pub const COMPILE_NODES_SUBDIR: &str = "compile/nodes";
pub const COMPILE_COLUMNS_SUBDIR: &str = "compile/columns";
pub const COMPILE_CLL_SUBDIR: &str = "compile/column_lineage";
pub const CATALOG_COLUMNS_SUBDIR: &str = "catalog/columns";
pub const RUN_INVOCATIONS_SUBDIR: &str = "run/invocations";
pub const RUN_RESULTS_SUBDIR: &str = "run/results";
pub const RUN_FRESHNESS_SUBDIR: &str = "run/freshness";
pub const RUN_CATALOG_STATS_SUBDIR: &str = "run/catalog_stats";

// ---------------------------------------------------------------------------
// IngestState — tracks what epochs have been applied
// ---------------------------------------------------------------------------

/// Tracks what has been applied from `target/private/metadata/` into DuckDB.
///
/// ## Delta decision logic (per epoch directory)
///
/// Let `last` = stored last epoch number, `curr_max` = max epoch on disk.
///
/// | Condition              | Meaning                          | Action          |
/// |------------------------|----------------------------------|-----------------|
/// | `last == u32::MAX`     | First run, no state yet          | Full reload     |
/// | `curr_max < last`      | Compaction reset epoch numbering | Full reload     |
/// | `v1_0.parquet` mtime changed | Base epoch rewritten in place | Full reload     |
/// | `curr_max == last`     | No new epochs (may still have deletions) | Deletions only |
/// | `curr_max > last`      | New incremental epochs available | Delta load      |
///
/// The base-mtime check is load-bearing: a FullParse rewrites `v1_0.parquet` in
/// place and deletes the delta epochs, so epoch numbers alone cannot distinguish
/// "epoch 0 unchanged" from "epoch 0 rewritten with new content". Without it, any
/// change that triggers a FullParse (editing a `.yml` description, tags, meta, or
/// a `.md` docs block) is silently never ingested into the index.
///
/// The `run/*` subdirs need no mtime check. They do rewrite `v1_0.parquet` — via
/// consolidation once the file count passes 32 — but consolidation also deletes
/// every `n != 0` epoch, so `curr_max` drops to 0 while `last` is >32 and
/// `curr_max < last` already forces the reload. No run writer rewrites the base
/// epoch while leaving `curr_max` unchanged, which is the case mtime exists to catch.
///
/// Deletions are always computed via `alive_ids` diff regardless of epoch state:
/// a node removed from `alive.parquet` is deleted even if no epoch was written.
///
/// ## Trigger
/// `apply_delta` is a no-op unless `parse/alive.parquet` mtime changes — every
/// parse (cold start, incremental, compaction) rewrites alive.parquet last.
#[derive(Debug, Default)]
pub struct IngestState {
    /// mtime of `parse/alive.parquet` at last apply — the primary freshness signal.
    pub alive_mtime: Option<SystemTime>,
    /// Last epoch number applied per subdirectory. Defaults to `u32::MAX` (never seen).
    pub last_epoch: HashMap<&'static str, u32>,
    /// Directory where crack_epochs writes flat delta parquet files.
    pub index_dir: Option<PathBuf>,
    /// Alive unique_ids after the last apply. Diff against current alive.parquet
    /// gives the deleted set — no DuckDB table scan required.
    pub alive_ids: HashSet<String>,
    /// mtime of `{subdir}/v1_0.parquet` at last apply. Detects an in-place rewrite
    /// of the base epoch, which leaves epoch numbering unchanged.
    pub base_mtime: HashMap<&'static str, SystemTime>,
}

impl IngestState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn last_epoch_for(&self, subdir: &'static str) -> u32 {
        self.last_epoch.get(subdir).copied().unwrap_or(u32::MAX)
    }

    pub fn set_epoch(&mut self, subdir: &'static str, epoch: u32) {
        self.last_epoch.insert(subdir, epoch);
    }

    /// `(current base mtime, whether it differs from the recorded one)`, or `None`
    /// when the base epoch is absent or its mtime is unreadable.
    ///
    /// Compares at microsecond precision — persisted state is µs, OS mtime is ns.
    fn base_mtime_diff(&self, subdir: &'static str, dir: &Path) -> Option<(SystemTime, bool)> {
        let current = std::fs::metadata(dir.join(epoch_layers::base_epoch_filename()))
            .and_then(|m| m.modified())
            .ok()?;
        let stored = self.base_mtime.get(subdir).copied().and_then(mtime_us);
        Some((current, stored != mtime_us(current)))
    }

    /// Read-only form of [`Self::base_rewritten`] — reports whether the base epoch
    /// differs from the recorded mtime without updating it. For callers that only
    /// need to decide whether an ingest pass is worth running.
    pub fn base_differs(&self, subdir: &'static str, dir: &Path) -> bool {
        matches!(self.base_mtime_diff(subdir, dir), Some((_, true)))
    }

    /// Whether the base epoch in `dir` was rewritten since the last apply, recording
    /// the new mtime as a side effect.
    ///
    /// Returns `false` when the base epoch is absent, when its mtime is unreadable,
    /// or when the mtime is unchanged. On the first call for a subdir there is no
    /// stored mtime, so this returns `true` — harmless, because every caller is
    /// already doing a full reload on its first pass (`last == u32::MAX`).
    ///
    /// Must be called unconditionally rather than as the right-hand side of a
    /// short-circuiting `||`: skipping it leaves `base_mtime` unrecorded, so the
    /// *next* delta sees no stored mtime and needlessly reloads in full.
    pub fn base_rewritten(&mut self, subdir: &'static str, dir: &Path) -> bool {
        let Some((current, differs)) = self.base_mtime_diff(subdir, dir) else {
            return false;
        };
        if differs {
            self.base_mtime.insert(subdir, current);
        }
        differs
    }

    /// Whether `subdir` must be reloaded in full rather than by delta, given the max
    /// epoch number currently on disk. `force` is the caller's own override.
    ///
    /// Full reload when the caller forces it, on the first pass (`last == u32::MAX`),
    /// after a compaction reset the numbering (`curr_max < last`), or when the base
    /// epoch was rewritten in place — see the type-level docs for why each matters.
    ///
    /// Records the base mtime as a side effect even when the answer is already `true`,
    /// so a later delta doesn't reload in full for want of a stored mtime.
    pub fn needs_full_reload(
        &mut self,
        subdir: &'static str,
        dir: &Path,
        curr_max: u32,
        force: bool,
    ) -> bool {
        // Not folded into the `||` chain: short-circuiting would skip the recording.
        let rewritten = self.base_rewritten(subdir, dir);
        let last = self.last_epoch_for(subdir);
        force || last == u32::MAX || curr_max < last || rewritten
    }
}

/// `t` as microseconds since the Unix epoch, or `None` if `t` predates it.
pub fn mtime_us(t: SystemTime) -> Option<u64> {
    t.duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_micros() as u64)
}
