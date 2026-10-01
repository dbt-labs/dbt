use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use adbc_record_replay::{RecordConnection, RecordingContext, ReplayConnection};
use arrow_array::RecordBatch;
use dbt_adapter_core::AdapterType;
use dbt_adbc::{Backend, Connection, QueryCtx};
use dbt_auth::AdapterConfig;
use dbt_common::behavior_flags::Behavior;
use dbt_common::cancellation::CancellationToken;
use dbt_common::tracing::emit::emit_trace_event;
use dbt_schemas::schemas::common::ResolvedQuoting;
use dbt_telemetry::AdapterConnectionOpen;
use minijinja::State;

use crate::cache::RelationCache;
use crate::engine::query_comment::QueryCommentConfig;
use crate::sql_types::TypeOps;
use crate::stmt_splitter::StmtSplitter;

use super::adapter_engine::{AdapterEngine, Options};
use super::adbc::assert_connection_on_pool_worker;

static GENERATION: AtomicU64 = AtomicU64::new(1);

fn next_generation() -> u64 {
    GENERATION.fetch_add(1, Ordering::Relaxed)
}

enum Mode {
    Record,
    Replay,
}

pub struct RecordReplayEngine {
    inner: Arc<dyn AdapterEngine>,
    recordings_path: PathBuf,
    config: adbc_record_replay::SharedConfig,
    query_comment: Option<QueryCommentConfig>,
    mode: Mode,
    generation: u64,
}

impl RecordReplayEngine {
    pub fn record(inner: Arc<dyn AdapterEngine>, recordings_path: PathBuf) -> Self {
        let generation = next_generation();
        // A reset invalidates every connection from an earlier generation, and
        // `fingerprint()` *is* the generation, so any cached one is discarded on
        // the fingerprint mismatch when it is next borrowed.
        adbc_record_replay::reset_counters(&recordings_path);
        Self {
            inner,
            recordings_path,
            config: Arc::new(adbc_record_replay::Config {
                sql_normalizer: Box::new(DbtSqlNormalizer),
            }),
            query_comment: None,
            mode: Mode::Record,
            generation,
        }
    }

    pub fn replay(
        inner: Arc<dyn AdapterEngine>,
        recordings_path: PathBuf,
        query_comment: Option<QueryCommentConfig>,
    ) -> Self {
        let generation = next_generation();
        // A reset invalidates every connection from an earlier generation, and
        // `fingerprint()` *is* the generation, so any cached one is discarded on
        // the fingerprint mismatch when it is next borrowed.
        adbc_record_replay::reset_counters(&recordings_path);
        Self {
            inner,
            recordings_path,
            config: Arc::new(adbc_record_replay::Config {
                sql_normalizer: Box::new(DbtSqlNormalizer),
            }),
            query_comment,
            mode: Mode::Replay,
            generation,
        }
    }
}

impl AdapterEngine for RecordReplayEngine {
    fn adapter_type(&self) -> AdapterType {
        self.inner.adapter_type()
    }

    fn backend(&self) -> Backend {
        self.inner.backend()
    }

    fn quoting(&self) -> ResolvedQuoting {
        self.inner.quoting()
    }

    fn splitter(&self) -> &dyn StmtSplitter {
        self.inner.splitter()
    }

    fn type_ops(&self) -> &Arc<dyn TypeOps> {
        self.inner.type_ops()
    }

    fn query_comment(&self) -> &QueryCommentConfig {
        self.query_comment
            .as_ref()
            .unwrap_or_else(|| self.inner.query_comment())
    }

    fn config(&self, key: &str) -> Option<Cow<'_, str>> {
        self.inner.config(key)
    }

    fn get_config(&self) -> &AdapterConfig {
        self.inner.get_config()
    }

    fn relation_cache(&self) -> &Arc<RelationCache> {
        self.inner.relation_cache()
    }

    fn behavior(&self) -> &Arc<Behavior> {
        self.inner.behavior()
    }

    fn behavior_flag_overrides(&self) -> &BTreeMap<String, bool> {
        self.inner.behavior_flag_overrides()
    }

    fn is_replay(&self) -> bool {
        matches!(self.mode, Mode::Replay)
    }

    fn fingerprint(&self) -> u64 {
        self.generation
    }

    fn new_connection(
        &self,
        state: Option<&State>,
        node_id: Option<String>,
    ) -> dbt_common::AdapterResult<Box<dyn Connection>> {
        // Same contract as the live engine (`AdbcEngine::new_connection`).
        // Checked here rather than left to the `Mode::Record` delegation below,
        // so replayed runs -- which never reach an inner engine -- catch an
        // off-pool caller too.
        assert_connection_on_pool_worker();

        match self.mode {
            Mode::Replay => {
                let mut conn = ReplayConnection::new(
                    self.recordings_path.clone(),
                    self.config.clone(),
                    self.generation,
                );
                conn.set_recording_context(RecordingContext {
                    node_id,
                    metadata: false,
                });
                emit_trace_event(|| {
                    (
                        AdapterConnectionOpen {
                            adapter_type: self.adapter_type().as_ref().to_owned(),
                            adapter_backend: self.backend().to_string(),
                        }
                        .into(),
                        None,
                    )
                });
                Ok(Box::new(conn))
            }
            Mode::Record => {
                let inner = self.inner.new_connection(state, node_id.clone())?;
                let mut conn = RecordConnection::new(
                    self.recordings_path.clone(),
                    inner,
                    self.config.clone(),
                    self.generation,
                );
                conn.set_recording_context(RecordingContext {
                    node_id,
                    metadata: false,
                });
                Ok(Box::new(conn))
            }
        }
    }

    fn new_connection_with_config(
        &self,
        config: &AdapterConfig,
    ) -> dbt_common::AdapterResult<Box<dyn Connection>> {
        match self.mode {
            Mode::Replay => {
                let conn = ReplayConnection::new(
                    self.recordings_path.clone(),
                    self.config.clone(),
                    self.generation,
                );
                emit_trace_event(|| {
                    (
                        AdapterConnectionOpen {
                            adapter_type: self.adapter_type().as_ref().to_owned(),
                            adapter_backend: self.backend().to_string(),
                        }
                        .into(),
                        None,
                    )
                });
                Ok(Box::new(conn))
            }
            Mode::Record => self.inner.new_connection_with_config(config),
        }
    }

    fn execute_with_options(
        &self,
        state: Option<&State>,
        ctx: &QueryCtx,
        conn: &'_ mut dyn Connection,
        sql: &str,
        options: Options,
        fetch: bool,
        token: CancellationToken,
    ) -> dbt_common::AdapterResult<RecordBatch> {
        super::adapter_engine::adbc_execute_with_options(
            self, state, ctx, conn, sql, options, fetch, token,
        )
    }
}

struct DbtSqlNormalizer;

/// Matches the token name `mint_snowflake_catalog_credential` mints
/// (`compute_platform.rs`'s `token_name`), e.g. `dbt_compute_1725000000000`.
/// It embeds a real wall-clock millisecond timestamp, so record and replay
/// runs never mint the literal same name -- mask it out for comparison the
/// same way ephemeral warehouse names already are, below.
static DBT_COMPUTE_TOKEN_NAME: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"dbt_compute_\d+").unwrap());

/// Matches the quoted Snowflake username in `alter user "<user>" add/remove
/// programmatic access token ...` (`mint_snowflake_catalog_credential` /
/// `drop_minted_token` in `compute_platform.rs`). The user is whichever real
/// Snowflake identity recorded the fixture, which will never match another
/// engineer's identity or the `fake_user`/`FAKE_USER` fallback used at
/// pure-replay time -- mask it out the same way as the timestamp above.
/// Mirrors `cleanup_alter_user_identifier` in `adbc-record-replay`'s
/// `naming.rs`, which does the same masking for the recording lookup key;
/// this one is for the post-lookup text-equality check below.
static ALTER_USER_IDENTIFIER: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r#"(?i)(alter user )"[^"]+""#).unwrap());

/// Matches the temp table names of the ClickHouse `EXCHANGE TABLES` capability
/// probe (`can_exchange` in `metadata/clickhouse`), e.g.
/// `__dbt_exchange_test_0_31337_1788805827133081759`. The suffix is a process
/// id plus a wall-clock nanosecond timestamp, so record and replay runs never
/// emit the literal same name -- mask it out the same way as the timestamp
/// above. Mirrors `cleanup_exchange_probe_tables` in `adbc-record-replay`'s
/// `naming.rs`, which does the same masking for the recording lookup key;
/// this one is for the post-lookup text-equality check below.
static EXCHANGE_PROBE_TABLE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"__dbt_exchange_test_(\d+)_\d+_\d+").unwrap());

/// Matches the quoted Snowflake username in `show user programmatic access
/// tokens for user "<user>"` (`pat_hygiene_report` in `compute_platform.rs`).
/// Mirrors `cleanup_show_user_pat_identifier` in `adbc-record-replay`'s
/// `naming.rs`, which does the same masking for the recording lookup key;
/// this one is for the post-lookup text-equality check below.
static SHOW_USER_PAT_IDENTIFIER: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?i)(show user programmatic access tokens for user )"[^"]+""#).unwrap()
    });

/// Matches `dbt debug`'s MDLS write/read-back probe table name
/// (`__dbt_debug_probe_<nanos>`, `debug_mdls.rs`), e.g.
/// `__dbt_debug_probe_1789002662171033000`. A fresh wall-clock nanosecond
/// timestamp generated on every invocation, record or replay alike, so no
/// two runs ever emit the literal same name -- mask it out the same way as
/// the timestamp above. Mirrors `cleanup_debug_probe_table` in
/// `adbc-record-replay`'s `naming.rs`, which does the same masking for the
/// recording lookup key; this one is for the post-lookup text-equality
/// check below.
static DEBUG_PROBE_TABLE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"__dbt_debug_probe_\d+").unwrap());

/// Matches `dbt debug`'s Snowflake propagation probe table name
/// (`__dbt_debug_propagation_<nanos>`, `debug_propagation.rs`). Same
/// reasoning as `DEBUG_PROBE_TABLE` above: a fresh wall-clock nanosecond
/// timestamp on every invocation, record or replay alike, so no two runs
/// ever emit the literal same name -- mask it out for comparison. Mirrors
/// `cleanup_debug_propagation_probe_table` in `adbc-record-replay`'s
/// `naming.rs`, which does the same masking for the recording lookup key;
/// this one is for the post-lookup text-equality check below.
static DEBUG_PROPAGATION_PROBE_TABLE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"__dbt_debug_propagation_\d+").unwrap());

impl adbc_record_replay::SqlNormalizer for DbtSqlNormalizer {
    fn normalize(&self, sql: &str) -> String {
        use crate::sql::normalize::normalize_dbt_tmp_name;
        let normalized = normalize_dbt_tmp_name(sql);
        let collapsed = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
        let collapsed = DBT_COMPUTE_TOKEN_NAME
            .replace_all(&collapsed, "dbt_compute_[MASKED_TS]")
            .into_owned();
        let collapsed = ALTER_USER_IDENTIFIER
            .replace_all(&collapsed, r#"$1"[MASKED_USER]""#)
            .into_owned();
        let collapsed = EXCHANGE_PROBE_TABLE
            .replace_all(&collapsed, "__dbt_exchange_test_${1}_[MASKED_ID]")
            .into_owned();
        let collapsed = SHOW_USER_PAT_IDENTIFIER
            .replace_all(&collapsed, r#"$1"[MASKED_USER]""#)
            .into_owned();
        let collapsed = DEBUG_PROBE_TABLE
            .replace_all(&collapsed, "__dbt_debug_probe_[MASKED_ID]")
            .into_owned();
        let collapsed = DEBUG_PROPAGATION_PROBE_TABLE
            .replace_all(&collapsed, "__dbt_debug_propagation_[MASKED_ID]")
            .into_owned();
        collapsed
            .replace("DBT_TESTING_ALT", "[MASKED_ALT_WH]")
            .replace("DBT_TESTING", "[MASKED_WH]")
            .replace("FUSION_ADAPTER_TESTING", "[MASKED_WH]")
            .replace("FUSION_SLT_WAREHOUSE", "[MASKED_WH]")
    }
}
