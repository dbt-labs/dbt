pub(crate) mod error;
mod naming;
mod record;
mod replay;
mod storage;

pub use naming::cleanup_schema_name;
pub use record::RecordConnection;
pub use replay::ReplayConnection;

use adbc_core::options::OptionValue;
use dashmap::{DashMap, DashSet};
use once_cell::sync::Lazy;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Per-directory sequence counters for record/replay.
///
/// Keyed by recordings directory path so that multiple adapters and
/// sequential invocations each get their own counter namespace.
static COUNTERS: Lazy<DashMap<PathBuf, DashMap<String, usize>>> = Lazy::new(DashMap::new);

/// Paths that should NOT have their counters reset between sequential
/// invocations.
static SKIP_RESET_PATHS: Lazy<DashSet<PathBuf>> = Lazy::new(DashSet::new);

/// Mark a recording path to skip counter resets between sequential
/// invocations. Useful for multi-command tests where the same model
/// runs in different steps with different schemas.
pub fn skip_counter_reset(path: &Path) {
    SKIP_RESET_PATHS.insert(path.to_path_buf());
}

/// Clear sequence counters for the given recordings directory.
///
/// Call when a new record/replay session starts so that each session
/// begins with fresh sequence numbers. Returns `true` if counters
/// were actually cleared (i.e. the path was not in the skip set).
pub fn reset_counters(path: &Path) -> bool {
    if SKIP_RESET_PATHS.contains(path) {
        return false;
    }
    COUNTERS.remove(path);
    true
}

/// User-supplied SQL normalizer for replay comparison.
///
/// During replay, the recorded SQL and the incoming SQL are both passed
/// through this normalizer before comparison. Implement this trait to
/// strip environment-specific identifiers (e.g. dbt tmp UUIDs,
/// warehouse names) so recordings are portable.
pub trait SqlNormalizer: Send + Sync {
    fn normalize(&self, sql: &str) -> String;
}

/// Default normalizer that only collapses whitespace.
struct DefaultSqlNormalizer;

impl SqlNormalizer for DefaultSqlNormalizer {
    fn normalize(&self, sql: &str) -> String {
        sql.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

/// Per-query context that record/replay needs to name recording files.
///
/// Set on a [`RecordConnection`] or [`ReplayConnection`] via
/// [`set_recording_context`](RecordConnection::set_recording_context)
/// before creating statements. Each statement inherits the context
/// that was active on the connection at creation time.
#[derive(Clone, Debug, Default)]
pub struct RecordingContext {
    pub node_id: Option<String>,
    pub metadata: bool,
}

impl RecordingContext {
    /// Absorb a statement option that is relevant to recording context.
    ///
    /// Returns `true` if the option was recognized and applied and `false` otherwise.
    pub fn absorb_option(&mut self, name: &str, value: &OptionValue) -> bool {
        match name {
            "dbt.node_id" => {
                if let OptionValue::String(node_id) = value {
                    self.node_id = Some(node_id.clone());
                }
                true
            }
            "dbt.metadata" => {
                if let OptionValue::Int(v) = value {
                    self.metadata = *v != 0;
                }
                true
            }
            _ => false,
        }
    }
}

/// Configuration for a record/replay session.
pub struct Config {
    /// SQL normalizer used when comparing recorded vs replayed queries.
    pub sql_normalizer: Box<dyn SqlNormalizer>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sql_normalizer: Box::new(DefaultSqlNormalizer),
        }
    }
}

impl Config {
    pub(crate) fn normalize_sql(&self, sql: &str) -> String {
        self.sql_normalizer.normalize(sql)
    }
}

pub type SharedConfig = Arc<Config>;

/// Debug-print the recordings at `recordings_dir` into `stream`
pub fn debug_recording_file(
    mut stream: impl std::io::Write,
    recordings_dir: &Path,
) -> Result<(), String> {
    let handler = storage::sqlite::SqliteHandler::new(recordings_dir);
    let rows = match handler.read_all_rows() {
        Ok(r) => r,
        Err(e) => {
            return Err(format!("error: {e}"));
        }
    };

    for row in rows {
        write!(stream, "{:#?}\n=====================\n", row)
            .map_err(|e| format!("Failed to write to stream: {e}"))?;
    }

    Ok(())
}
