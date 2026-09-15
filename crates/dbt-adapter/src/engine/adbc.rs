use std::borrow::Cow;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::Schema;
use dbt_adapter_core::AdapterType;
use dbt_adbc::*;
use dbt_agate::hashers::IdentityBuildHasher;
use dbt_auth::{AdapterConfig, Auth, AuthError};
use dbt_common::AdapterResult;
use dbt_common::behavior_flags::Behavior;
use dbt_common::cancellation::CancellationToken;
use dbt_common::tracing::emit::emit_trace_event;
use dbt_schemas::schemas::common::ResolvedQuoting;
use dbt_telemetry::AdapterConnectionOpen;
use minijinja::State;
use parking_lot::RwLock;

use crate::cache::RelationCache;
use crate::engine::query_comment::QueryCommentConfig;
use crate::errors::{AdapterError, adbc_error_to_adapter_error};
use crate::sql_types::TypeOps;
use crate::stmt_splitter::StmtSplitter;

use super::adapter_engine::*;
use super::databricks;
use super::make_behavior;
use super::noop_connection::NoopConnection;
use super::retry::ConnectionRetryPolicy;

#[derive(Default)]
pub struct DatabaseMap {
    inner: HashMap<database::Fingerprint, Box<dyn Database>, IdentityBuildHasher>,
}

/// Operational mode for [`AdbcEngine`].
///
/// Controls how the engine creates connections and executes queries.
#[derive(Debug)]
pub enum EngineMode {
    /// Normal ADBC execution against a live warehouse.
    Live,
    /// Stubbed connections and execution
    Mock,
}

impl EngineMode {
    pub fn has_real_connections(&self) -> bool {
        matches!(self, EngineMode::Live)
    }
}

pub struct AdbcEngine {
    adapter_type: AdapterType,
    /// Auth configurator
    auth: Arc<dyn Auth>,
    /// Configuration
    config: AdapterConfig,
    /// Lazily initialized databases
    configured_databases: RwLock<DatabaseMap>,
    /// Resolved quoting policy
    quoting: ResolvedQuoting,
    /// Query comment config
    query_comment: QueryCommentConfig,
    /// Type operations (e.g. parsing, formatting) for the dialect this engine is for
    pub type_ops: Arc<dyn TypeOps>,
    /// Statement splitter
    splitter: Arc<dyn StmtSplitter>,
    /// Relation cache - caches warehouse relation metadata to avoid repeated queries
    relation_cache: Arc<RelationCache>,
    /// User overrides for behavior flags from dbt_project.yml
    behavior_flag_overrides: BTreeMap<String, bool>,
    /// Resolved behavior object with user overrides applied
    behavior: Arc<Behavior>,
    /// Controls connection/execution behaviour.
    mode: EngineMode,
    /// The `threads` configuration value from the dbt profile.
    threads: Option<usize>,
    /// Config fingerprint identifying this engine's connections. Used by the
    /// pool to reuse a connection only among engines with an identical
    /// connection configuration (not merely the same engine instance). Lazily
    /// set: `0` until the first real connection is created, then the config
    /// fingerprint of that connection.
    connection_fingerprint: std::sync::atomic::AtomicU64,
    /// `ResolvedCloudConfig::project_id`, forwarded as the `dbt_cloud.project_id`
    /// flock-adbc driver option alongside whichever credential is resolved.
    dbt_cloud_project_id: Option<String>,
}

impl AdbcEngine {
    #[allow(clippy::too_many_arguments)]
    fn build(
        adapter_type: AdapterType,
        auth: Arc<dyn Auth>,
        config: AdapterConfig,
        quoting: ResolvedQuoting,
        query_comment: QueryCommentConfig,
        type_ops: Arc<dyn TypeOps>,
        splitter: Arc<dyn StmtSplitter>,
        relation_cache: Arc<RelationCache>,
        behavior_flag_overrides: BTreeMap<String, bool>,
        mode: EngineMode,
        threads: Option<usize>,
        dbt_cloud_project_id: Option<String>,
    ) -> Self {
        let behavior = make_behavior(adapter_type, &behavior_flag_overrides);
        Self {
            adapter_type,
            auth,
            config,
            quoting,
            configured_databases: RwLock::new(DatabaseMap::default()),
            type_ops,
            splitter,
            query_comment,
            relation_cache,
            behavior_flag_overrides,
            behavior,
            mode,
            threads,
            connection_fingerprint: std::sync::atomic::AtomicU64::new(0),
            dbt_cloud_project_id,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        adapter_type: AdapterType,
        auth: Arc<dyn Auth>,
        config: AdapterConfig,
        quoting: ResolvedQuoting,
        query_comment: QueryCommentConfig,
        type_ops: Arc<dyn TypeOps>,
        splitter: Arc<dyn StmtSplitter>,
        relation_cache: Arc<RelationCache>,
        behavior_flag_overrides: BTreeMap<String, bool>,
        threads: Option<usize>,
        dbt_cloud_project_id: Option<String>,
    ) -> Self {
        Self::build(
            adapter_type,
            auth,
            config,
            quoting,
            query_comment,
            type_ops,
            splitter,
            relation_cache,
            behavior_flag_overrides,
            EngineMode::Live,
            threads,
            dbt_cloud_project_id,
        )
    }

    /// Create a mock engine that stubs out connections and execution.
    ///
    /// Used for replay modes and test adapters that must never talk to a
    /// real warehouse.
    #[allow(clippy::too_many_arguments)]
    pub fn new_mock(
        adapter_type: AdapterType,
        auth: Arc<dyn Auth>,
        config: AdapterConfig,
        quoting: ResolvedQuoting,
        type_ops: Arc<dyn TypeOps>,
        splitter: Arc<dyn StmtSplitter>,
        relation_cache: Arc<RelationCache>,
        behavior_flag_overrides: BTreeMap<String, bool>,
    ) -> Self {
        Self::build(
            adapter_type,
            auth,
            config,
            quoting,
            QueryCommentConfig::from_query_comment(None, adapter_type, false, None),
            type_ops,
            splitter,
            relation_cache,
            behavior_flag_overrides,
            EngineMode::Mock,
            None,
            None,
        )
    }

    /// Get the engine mode.
    pub fn mode(&self) -> &EngineMode {
        &self.mode
    }

    fn load_driver_and_configure_database(
        &self,
        config: &AdapterConfig,
    ) -> AdapterResult<(Box<dyn Database>, database::Fingerprint)> {
        assert!(
            self.mode.has_real_connections(),
            "load_driver_and_configure_database called in {:?} mode",
            self.mode,
        );
        let use_cloud_credentials = config.use_dbt_cloud_credentials();
        let backend = self.auth.backend();

        let database_builder = config
            .build_connection_builder(self.auth.as_ref(), |backend| {
                self.configure_cloud_database(backend)
            })
            .map_err(crate::errors::auth_error_to_adapter_error)?;
        let load_strategy = match (use_cloud_credentials, self.adapter_type) {
            (true, _) => LoadStrategy::Remote,
            (false, AdapterType::DuckDB) => LoadStrategy::SystemThenCdnCache,
            (false, _) => LoadStrategy::CdnCache,
        };

        // This will load the "flock" driver if load_strategy is Remote.
        let mut driver = driver::Builder::new(backend, load_strategy)
            .try_load()
            .map_err(adbc_error_to_adapter_error)?;

        // The database is configured only once even if this runs multiple times,
        // unless a different configuration is provided.
        let opts = database_builder.into_iter().collect::<Vec<_>>();
        let fingerprint = database::Builder::fingerprint(opts.iter());
        {
            let read_guard = self.configured_databases.read();
            if let Some(database) = read_guard.inner.get(&fingerprint) {
                return Ok((database.clone(), fingerprint));
            }
        }
        {
            let mut write_guard = self.configured_databases.write();
            if let Some(database) = write_guard.inner.get(&fingerprint) {
                let database: Box<dyn Database> = database.clone();
                Ok((database, fingerprint))
            } else {
                let database = driver
                    .new_database_with_opts(opts)
                    .map_err(adbc_error_to_adapter_error)?;
                match self.adapter_type {
                    AdapterType::DuckDB => self.apply_duckdb_init_sql(database.as_ref(), config)?,
                    AdapterType::ClickHouse => {
                        // temporary connection with no current schema set
                        let mut conn = database
                            .new_connection()
                            .map_err(adbc_error_to_adapter_error)?;
                        super::clickhouse::ensure_database(conn.as_mut(), config)?
                    }
                    _ => {}
                }
                write_guard.inner.insert(fingerprint, database.clone());
                Ok((database, fingerprint))
            }
        }
    }

    /// Build a [database::Builder] configured with dbt platform credentials, resolved via
    /// `dbt-platform-auth`'s credential chain. Platform credentials are only used when the
    /// user has explicitly opted in via configuration (`use_dbt_cloud_credentials`), so a
    /// resolution failure here is a hard configuration error rather than a silent no-op.
    fn configure_cloud_database(&self, backend: Backend) -> Result<database::Builder, AuthError> {
        Self::configure_cloud_database_with_chain(
            backend,
            dbt_platform_auth::AuthChainBuilder::default().build(),
            self.dbt_cloud_project_id.as_deref(),
        )
    }

    /// Split out of [`Self::configure_cloud_database`] so tests can supply an `AuthChain`
    /// pointed at a fixture instead of the real `~/.dbt/*`.
    fn configure_cloud_database_with_chain(
        backend: Backend,
        chain: dbt_platform_auth::AuthChain,
        project_id: Option<&str>,
    ) -> Result<database::Builder, AuthError> {
        let credential: Result<dbt_platform_auth::Credential, dbt_platform_auth::AuthError> =
            dbt_common::tracing::spawn_traced_block_in_place(async move { chain.resolve().await });
        Self::apply_cloud_credential(backend, credential, project_id)
    }

    /// Pure helper split out of [`Self::configure_cloud_database`] so the credential →
    /// driver-option mapping can be unit tested without needing a real `AuthChain`/
    /// `~/.dbt/*` on disk.
    fn apply_cloud_credential(
        backend: Backend,
        credential: Result<dbt_platform_auth::Credential, dbt_platform_auth::AuthError>,
        project_id: Option<&str>,
    ) -> Result<database::Builder, AuthError> {
        use dbt_platform_auth::Credential;

        let mut builder = database::Builder::new(backend);
        let credential = credential.map_err(|e| {
            let hint = e
                .login_hint()
                .map(|h| format!(" — {h}"))
                .unwrap_or_default();
            AuthError::config(format!("{e}{hint}"))
        })?;

        // See NOTE [flock-service-token-support] in flock-service/src/flight/handshake.rs:
        // flock-service doesn't support service-credential fetching yet, so reject this
        // client-side instead of sending a token the server will reject anyway.
        if matches!(credential, Credential::ServiceToken { .. }) {
            return Err(AuthError::config(
                "service token credentials are not yet supported for dbt Cloud/flock \
                 connections; use a personal access token or run `dbt login` instead",
            ));
        }

        builder.with_named_option("dbt_cloud.token", credential.token())?;
        builder.with_named_option(
            "dbt_cloud.host",
            flock_driver_host(credential.account_host())?,
        )?;
        builder.with_named_option("dbt_cloud.account_id", credential.account_id().to_string())?;
        if let Some(project_id) = project_id {
            builder.with_named_option("dbt_cloud.project_id", project_id)?;
        }
        Ok(builder)
    }

    /// Apply DuckDB init SQL (extensions, settings, secrets, attachments)
    /// to a newly created database instance. Uses a temporary connection.
    fn apply_duckdb_init_sql(
        &self,
        database: &dyn Database,
        config: &AdapterConfig,
    ) -> AdapterResult<()> {
        let mut all_stmts = dbt_auth::generate_duckdb_init_sql(config)
            .map_err(crate::errors::auth_error_to_adapter_error)?;

        // Append v2 catalog-driven ATTACH statements for DuckDB REST catalogs
        all_stmts.extend(self.generate_v2_catalog_attach_stmts()?);

        if all_stmts.is_empty() {
            return Ok(());
        }
        let mut conn = database
            .new_connection()
            .map_err(adbc_error_to_adapter_error)?;
        for (idx, sql) in all_stmts.iter().enumerate() {
            let mut stmt = conn.new_statement().map_err(adbc_error_to_adapter_error)?;
            stmt.set_sql_query(sql)
                .map_err(adbc_error_to_adapter_error)?;
            let _ = stmt.execute_update().map_err(|e| {
                adbc_error_to_adapter_error(adbc_core::error::Error::with_message_and_status(
                    format!("DuckDB init SQL statement {} failed: {e}", idx + 1),
                    adbc_core::error::Status::Internal,
                ))
            })?;
        }
        Ok(())
    }

    /// Build v2 catalog-driven `ATTACH IF NOT EXISTS` statements for DuckDB
    /// Horizon, Glue, Iceberg REST, Unity Catalog, and DuckLake catalogs.
    ///
    /// Reads the global catalogs v2 state, extracts every catalog that has a
    /// `config.duckdb` block, and emits one ATTACH per catalog. Duplicate
    /// aliases (after sanitization) are rejected with an error.
    fn generate_v2_catalog_attach_stmts(&self) -> AdapterResult<Vec<String>> {
        use crate::load_catalogs;

        if !load_catalogs::fetch_use_catalogs_v2() {
            return Ok(Vec::new());
        }
        let Some(catalogs) = load_catalogs::fetch_catalogs() else {
            return Ok(Vec::new());
        };
        let Ok(view) = catalogs.view_v2() else {
            return Ok(Vec::new());
        };
        // The compute engine attaches via each catalog's `lake_compute`
        // block when present; the base DuckDB adapter uses the `duckdb` block.
        // Both fall back to `duckdb`.
        let platform = if self.adapter_type == AdapterType::LakeCompute {
            AdapterType::LakeCompute.as_ref()
        } else {
            AdapterType::DuckDB.as_ref()
        };
        super::duckdb_attach::compose_v2_catalog_attach_stmts(&view, platform)
    }
}

/// The flock-adbc driver connects through a dedicated `dwg.` (data warehouse
/// gateway) subdomain that only exists for flock traffic — every other
/// consumer of `credential.account_host()` (Cloud Config gRPC, the identify
/// service, `dbt_cloud.yml` matching, OAuth) must keep using the bare host.
fn flock_driver_host(account_host: &str) -> Result<String, AuthError> {
    if account_host.split('.').any(|label| label == "dwg") {
        return Ok(account_host.to_string());
    }
    dbt_common::url::insert_gateway_label(account_host, "dwg")
        .map_err(|e| AuthError::config(format!("invalid dbt Cloud host {account_host:?}: {e}")))
}

/// Ignored under dbt-platform brokered credentials — per-model routing
/// doesn't apply there.
pub(crate) fn resolve_connection_config<'a>(
    adapter_type: AdapterType,
    base_config: &'a AdapterConfig,
    state: Option<&State>,
) -> Cow<'a, AdapterConfig> {
    match adapter_type {
        AdapterType::Databricks if base_config.contains_key("compute") => {
            match state.and_then(databricks::compute_from_state) {
                Some(databricks_compute) => {
                    let mut mapping = base_config.repr().clone();
                    mapping.insert("databricks_compute".into(), databricks_compute.into());
                    Cow::Owned(AdapterConfig::new(mapping))
                }
                None => Cow::Borrowed(base_config),
            }
        }
        _ => Cow::Borrowed(base_config),
    }
}

/// Asserts that the caller is running on a dbt-runtime pool worker, as every
/// connection-opening path must.
///
/// When this assertion is violated, it means that a database connection
/// (consequently database work) is being performed in code that is not
/// triggered by a `dbt_runtime::spawn_blocking` call and this is a BUG.
///
/// This is a bug because to enforce `--threads` and in general, not overload
/// the databases with dbt work, we must run all the jinja and database work
/// in the dbt-runtime thread-pool.
pub(super) fn assert_connection_on_pool_worker() {
    if cfg!(debug_assertions) && !dbt_runtime::is_pool_worker() {
        off_pool_connection_bug();
    }
}

/// Reports the off-pool connection bug detected in [`AdbcEngine::new_connection`]
/// and unwinds.
///
/// Panics rather than aborts so the report survives a test harness: the dbt-cli
/// suite `dup2`s both stdout and stderr to files while the CLI runs, so an
/// `abort()` here left nothing behind but `signal 6 (SIGABRT)` -- the message
/// went to a file the harness only dumps when it catches a panic. Unwinding
/// gets that dump *and* carries the reason as the panic payload.
#[cold]
#[inline(never)]
fn off_pool_connection_bug() -> ! {
    let thread = std::thread::current();
    let name = thread.name().unwrap_or("<unnamed>").to_owned();
    let report = format!(
        "database connections MUST only be created by dbt-runtime worker threads, \
         but this one was created on thread `{name}` ({:?}).\n\
         \n\
         All jinja and database work has to run on the dbt-runtime blocking pool: \
         connections are thread-locals of its workers, which is what makes \
         `--threads` bound the work reaching the warehouse. Fix the caller, not \
         this assertion -- and see the frame below the adapter for which caller \
         that is:\n\
         \n\
         - production code in an `async fn`: move the call into \
         `dbt_runtime::spawn_blocking(..)` (or `TaskOp::Blocking`) and await it\n\
         - a synchronous `#[test]`: use `#[dbt_runtime::worker_test]`\n\
         - an `async` test: keep `#[dbt_runtime::test]` and dispatch the adapter \
         call with `dbt_runtime::spawn_blocking(..).await`\n\
         - a test helper that already hops threads: have the *helper* call \
         `dbt_runtime::testing::block_on_worker`",
        thread.id()
    );
    // Written here as well as raised: a `catch_unwind` between this frame and
    // the top would otherwise be free to swallow the whole report.
    let bt = std::backtrace::Backtrace::force_capture();
    eprintln!("FATAL: {report}\n{bt}");
    panic!("{report}");
}

impl AdapterEngine for AdbcEngine {
    #[inline]
    fn adapter_type(&self) -> AdapterType {
        self.adapter_type
    }

    fn backend(&self) -> Backend {
        self.auth.backend()
    }

    fn threads(&self) -> Option<usize> {
        self.threads
    }

    fn fingerprint(&self) -> u64 {
        self.connection_fingerprint
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    fn is_mock(&self) -> bool {
        matches!(self.mode, EngineMode::Mock)
    }

    fn quoting(&self) -> ResolvedQuoting {
        self.quoting
    }

    fn splitter(&self) -> &dyn StmtSplitter {
        self.splitter.as_ref()
    }

    fn type_ops(&self) -> &Arc<dyn TypeOps> {
        &self.type_ops
    }

    fn query_comment(&self) -> &QueryCommentConfig {
        &self.query_comment
    }

    fn config(&self, key: &str) -> Option<Cow<'_, str>> {
        self.config.get_string(key)
    }

    fn get_config(&self) -> &AdapterConfig {
        &self.config
    }

    fn relation_cache(&self) -> &Arc<RelationCache> {
        &self.relation_cache
    }

    fn new_connection(
        &self,
        state: Option<&State>,
        _node_id: Option<String>,
    ) -> AdapterResult<Box<dyn Connection>> {
        assert_connection_on_pool_worker();

        match &self.mode {
            EngineMode::Mock => {
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
                Ok(Box::new(NoopConnection))
            }
            EngineMode::Live => {
                let config = resolve_connection_config(self.adapter_type, &self.config, state);
                self.new_connection_with_config(config.as_ref())
            }
        }
    }

    /// Fingerprints `config` without opening a connection, so the pool can
    /// decide reuse before creating one. Must stay I/O-free.
    fn fingerprint_for_config(&self, config: &AdapterConfig) -> AdapterResult<u64> {
        match self.mode {
            // Mock mode never connects, so mock configs don't need real auth data.
            EngineMode::Mock => Ok(self.fingerprint()),
            EngineMode::Live => {
                let builder = config
                    .build_connection_builder(self.auth.as_ref(), |backend| {
                        self.configure_cloud_database(backend)
                    })
                    .map_err(crate::errors::auth_error_to_adapter_error)?;
                let opts = builder.into_iter().collect::<Vec<_>>();
                Ok(database::Builder::fingerprint(opts.iter()).as_u64())
            }
        }
    }

    fn new_connection_with_config(
        &self,
        config: &AdapterConfig,
    ) -> AdapterResult<Box<dyn Connection>> {
        if !self.mode.has_real_connections() {
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
            return Ok(Box::new(NoopConnection));
        }
        let (mut database, fingerprint) = self.load_driver_and_configure_database(config)?;
        let connect = || {
            let mut builder = connection::Builder::default();
            match self.adapter_type {
                // dbclient.py `_set_client_database` parity; ensure_database
                // guarantees it exists.
                AdapterType::ClickHouse => {
                    if let Some(schema) = super::clickhouse::target_schema(config) {
                        builder.with_option(
                            adbc_core::options::OptionConnection::CurrentSchema,
                            schema.as_ref(),
                        )?;
                    }
                }
                // dbt-gizmosql (Python) parity: the profile's `database` is the
                // DuckDB catalog made current for the Flight SQL session, so
                // unqualified names in user SQL resolve against it.
                AdapterType::GizmoSQL => {
                    if let Some(catalog) = config.get_string("database")
                        && !catalog.is_empty()
                    {
                        builder.with_option(
                            adbc_core::options::OptionConnection::CurrentCatalog,
                            catalog.as_ref(),
                        )?;
                    }
                }
                _ => {}
            }
            builder.build(&mut database)
        };
        let retry_policy = ConnectionRetryPolicy::new(self.adapter_type(), config);
        let mut conn = retry_policy
            .execute(config, connect)
            .map_err(|e| enrich_connection_error(self.adapter_type(), e, config))?;
        // Tag the connection with its config fingerprint and cache it on the
        // engine, so the pool reuses a connection only among engines with an
        // identical connection configuration.
        let fp = fingerprint.as_u64();
        conn.set_fingerprint(fp);
        self.connection_fingerprint
            .store(fp, std::sync::atomic::Ordering::Relaxed);
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
        Ok(conn)
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
    ) -> AdapterResult<RecordBatch> {
        if matches!(self.mode, EngineMode::Mock) {
            return Ok(RecordBatch::new_empty(Arc::new(Schema::empty())));
        }
        adbc_execute_with_options(self, state, ctx, conn, sql, options, fetch, token)
    }

    fn behavior(&self) -> &Arc<Behavior> {
        &self.behavior
    }

    fn behavior_flag_overrides(&self) -> &BTreeMap<String, bool> {
        &self.behavior_flag_overrides
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Enrich connection errors with adapter-specific hints where possible.
fn enrich_connection_error(
    adapter_type: AdapterType,
    err: adbc_core::error::Error,
    config: &AdapterConfig,
) -> AdapterError {
    use AdapterType::*;
    match adapter_type {
        // If `err` looks like a Snowflake HTTP 403 connection failure, replace
        // its message with one that hints at a misconfigured account identifier.
        // Other errors are returned unchanged.
        //
        // We key off HTTP 403 in the error message because that is the specific
        // status Snowflake returns when the account subdomain is not recognized.
        // The Go ADBC driver does not expose a dedicated vendor code for this
        // case (the error arrives as a raw HTTP failure, not a typed
        // SnowflakeError), so substring matching on the status code is the most
        // reliable signal available.
        Snowflake if err.message.contains(": 403") => {
            let account_display = config
                .get_string("account")
                .map(|a| format!("'{a}'"))
                .unwrap_or_else(|| "<unknown>".to_string());
            let message = format!(
                "Could not connect to Snowflake. One possible cause is an incorrect \
account identifier ({account_display}).\n\n\
If the 'account' field in your profile is wrong, the value should be \
in the format '<orgname>-<account_name>' (e.g. 'myorg-myaccount') and \
must not include '.snowflakecomputing.com'.\n\n\
You can find your account identifier in Snowsight under \
Admin > Accounts, or by running:\n  \
SELECT CURRENT_ORGANIZATION_NAME() || '-' || CURRENT_ACCOUNT_NAME()\n\n\
See: https://docs.snowflake.com/en/user-guide/admin-account-identifier#requirements-for-account-identifiers\n\n\
Original error: {}",
                err.message
            );
            AdapterError::new(adbc_error_to_adapter_error(err).kind(), message)
        }
        _ => adbc_error_to_adapter_error(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_yaml::Mapping;
    use minijinja::Environment;
    use minijinja::value::Value;
    use std::collections::BTreeMap;

    fn config_with_compute_block() -> AdapterConfig {
        AdapterConfig::new(Mapping::from_iter([("compute".into(), true.into())]))
    }

    /// dbt_yaml's dunder-key flatten mechanism wants `databricks_attr` as a
    /// sibling at the model's top level, not nested under `__adapter_attr__`.
    fn model_with_databricks_compute(compute: &str) -> Value {
        let databricks_attr = BTreeMap::from([("databricks_compute", compute)]);
        let model = BTreeMap::from([("databricks_attr", databricks_attr)]);
        Value::from_serialize(&model)
    }

    #[test]
    fn resolve_connection_config_non_databricks_ignores_compute_override() {
        let config = config_with_compute_block();
        let mut env = Environment::new();
        env.add_global("model", model_with_databricks_compute("large_warehouse"));
        let state = State::new_for_env(&env);

        let resolved = resolve_connection_config(AdapterType::Snowflake, &config, Some(&state));

        assert!(matches!(resolved, Cow::Borrowed(_)));
    }

    #[test]
    fn resolve_connection_config_databricks_without_compute_block_returns_borrowed() {
        let config = AdapterConfig::new(Mapping::new());
        let mut env = Environment::new();
        env.add_global("model", model_with_databricks_compute("large_warehouse"));
        let state = State::new_for_env(&env);

        let resolved = resolve_connection_config(AdapterType::Databricks, &config, Some(&state));

        assert!(matches!(resolved, Cow::Borrowed(_)));
    }

    #[test]
    fn resolve_connection_config_databricks_with_compute_block_no_state_returns_borrowed() {
        let config = config_with_compute_block();

        let resolved = resolve_connection_config(AdapterType::Databricks, &config, None);

        assert!(matches!(resolved, Cow::Borrowed(_)));
    }

    #[test]
    fn resolve_connection_config_databricks_with_compute_override_returns_owned() {
        let config = config_with_compute_block();
        let mut env = Environment::new();
        env.add_global("model", model_with_databricks_compute("large_warehouse"));
        let state = State::new_for_env(&env);

        let resolved = resolve_connection_config(AdapterType::Databricks, &config, Some(&state));

        match resolved {
            Cow::Owned(overridden) => {
                assert_eq!(
                    overridden.get_string("databricks_compute").as_deref(),
                    Some("large_warehouse")
                );
            }
            Cow::Borrowed(_) => {
                panic!("expected an overridden config with the compute override applied")
            }
        }
    }
}

#[cfg(test)]
mod cloud_credential_tests {
    use std::time::{Duration, SystemTime};

    use adbc_core::options::{OptionDatabase, OptionValue};
    use dbt_platform_auth::resolver::{AuthResolver, OAuthPassiveResolver};
    use dbt_platform_auth::{AuthChain, AuthError, Credential, OAuthSession, OAuthSessionCache};

    use super::AdbcEngine;
    use dbt_adbc::Backend;

    fn opt_string(opts: &[(OptionDatabase, OptionValue)], name: &str) -> Option<String> {
        opts.iter().find_map(|(key, value)| {
            let matches_name = matches!(key, OptionDatabase::Other(n) if n == name);
            if !matches_name {
                return None;
            }
            match value {
                OptionValue::String(s) => Some(s.clone()),
                _ => None,
            }
        })
    }

    fn oauth_credential() -> Credential {
        Credential::OAuth(OAuthSession {
            access_token: "access-token-123".to_string(),
            refresh_token: None,
            scopes: vec![],
            id_token: None,
            expires_at: SystemTime::now() + Duration::from_secs(3600),
            account_host: "ab123.us1.dbt.com".to_string(),
            account_id: 42,
            user_id: 7,
            client_id: "test-client".to_string(),
        })
    }

    #[test]
    fn oauth_credential_sets_token_host_account_id_and_project_id() {
        let builder = AdbcEngine::apply_cloud_credential(
            Backend::Snowflake,
            Ok(oauth_credential()),
            Some("proj-1"),
        )
        .expect("should succeed");
        let opts: Vec<_> = builder.into_iter().collect();
        assert_eq!(
            opt_string(&opts, "dbt_cloud.token"),
            Some("access-token-123".to_string())
        );
        assert_eq!(
            opt_string(&opts, "dbt_cloud.host"),
            Some("ab123.dwg.us1.dbt.com".to_string())
        );
        assert_eq!(
            opt_string(&opts, "dbt_cloud.account_id"),
            Some("42".to_string())
        );
        assert_eq!(
            opt_string(&opts, "dbt_cloud.project_id"),
            Some("proj-1".to_string())
        );
    }

    #[test]
    fn pat_credential_sets_token_host_account_id_with_no_project_id() {
        let credential = Credential::Pat {
            token: "dbtu_pat_token".to_string(),
            account_host: "ab123.us1.dbt.com".to_string(),
            account_id: 99,
        };
        let builder = AdbcEngine::apply_cloud_credential(Backend::Snowflake, Ok(credential), None)
            .expect("should succeed");
        let opts: Vec<_> = builder.into_iter().collect();
        assert_eq!(
            opt_string(&opts, "dbt_cloud.token"),
            Some("dbtu_pat_token".to_string())
        );
        assert_eq!(opt_string(&opts, "dbt_cloud.project_id"), None);
    }

    #[test]
    fn pat_credential_dbt_cloud_host_gets_dwg_label_without_mutating_account_host() {
        let credential = Credential::Pat {
            token: "dbtu_pat_token".to_string(),
            account_host: "ab123.us1.dbt.com".to_string(),
            account_id: 99,
        };
        // Confirm the credential's own account_host is left bare — only the
        // driver option gets the flock-specific "dwg" gateway label inserted.
        assert_eq!(credential.account_host(), "ab123.us1.dbt.com");
        let builder = AdbcEngine::apply_cloud_credential(Backend::Snowflake, Ok(credential), None)
            .expect("should succeed");
        let opts: Vec<_> = builder.into_iter().collect();
        assert_eq!(
            opt_string(&opts, "dbt_cloud.host"),
            Some("ab123.dwg.us1.dbt.com".to_string())
        );
    }

    #[test]
    fn pat_credential_dbt_cloud_host_is_not_double_prefixed() {
        let credential = Credential::Pat {
            token: "dbtu_pat_token".to_string(),
            account_host: "ab123.dwg.us1.dbt.com".to_string(),
            account_id: 99,
        };
        let builder = AdbcEngine::apply_cloud_credential(Backend::Snowflake, Ok(credential), None)
            .expect("should succeed");
        let opts: Vec<_> = builder.into_iter().collect();
        assert_eq!(
            opt_string(&opts, "dbt_cloud.host"),
            Some("ab123.dwg.us1.dbt.com".to_string())
        );
    }

    #[test]
    fn flock_driver_host_mc() {
        // Representative account-prefixed case; exhaustive host-shape coverage
        // for `insert_gateway_label` lives in `dbt_common::url`.
        assert_eq!(
            super::flock_driver_host("acme.dbt.com").unwrap(),
            "acme.dwg.dbt.com"
        );
    }

    #[test]
    fn service_token_credential_is_rejected() {
        let credential = Credential::ServiceToken {
            token: "dbtc_service_token".to_string(),
            account_host: "ab123.us1.dbt.com".to_string(),
            account_id: 99,
        };
        let err = AdbcEngine::apply_cloud_credential(Backend::Snowflake, Ok(credential), None)
            .expect_err("service tokens must be rejected");
        let message = err.msg();
        assert!(message.contains("service token"));
        assert!(message.contains("dbt login") || message.contains("personal access token"));
    }

    #[test]
    fn not_authenticated_surfaces_login_hint() {
        let err = AdbcEngine::apply_cloud_credential(
            Backend::Snowflake,
            Err(AuthError::NotAuthenticated),
            None,
        )
        .expect_err("no credentials must be a hard error, not a silent no-op");
        assert!(err.msg().contains("dbt login"));
    }

    #[test]
    fn malformed_error_propagates_without_a_login_hint() {
        let err = AdbcEngine::apply_cloud_credential(
            Backend::Snowflake,
            Err(AuthError::Malformed("bad yaml".to_string())),
            None,
        )
        .expect_err("malformed config must error");
        let message = err.msg();
        assert!(message.contains("bad yaml"));
        assert!(!message.contains("dbt login"));
    }

    #[dbt_runtime::test(flavor = "multi_thread")]
    async fn configure_cloud_database_with_chain_reads_seeded_oauth_session() {
        // No dbt_cloud.yml involved in this test — the OAuth session file is the only
        // credential source, and project_id is passed in directly.
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("oauth_sessions.json");
        let session = OAuthSession {
            access_token: "seeded-access-token".to_string(),
            refresh_token: None,
            scopes: vec![],
            id_token: None,
            expires_at: SystemTime::now() + Duration::from_secs(3600),
            account_host: "seeded.us1.dbt.com".to_string(),
            account_id: 555,
            user_id: 1,
            client_id: dbt_platform_auth::OAUTH_CLIENT_ID.to_string(),
        };
        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![session],
        };
        std::fs::write(&cache_path, serde_json::to_vec(&cache).unwrap()).unwrap();

        let mut resolver = OAuthPassiveResolver::new(dbt_platform_auth::OAUTH_CLIENT_ID);
        resolver.cache_path = Some(cache_path);
        let chain = AuthChain::new(vec![AuthResolver::OAuthPassive(resolver)]);

        let builder = AdbcEngine::configure_cloud_database_with_chain(
            Backend::Snowflake,
            chain,
            Some("proj-999"),
        )
        .expect("should resolve the seeded OAuth session");
        let opts: Vec<_> = builder.into_iter().collect();
        assert_eq!(
            opt_string(&opts, "dbt_cloud.token"),
            Some("seeded-access-token".to_string())
        );
        assert_eq!(
            opt_string(&opts, "dbt_cloud.host"),
            Some("seeded.dwg.us1.dbt.com".to_string())
        );
        assert_eq!(
            opt_string(&opts, "dbt_cloud.account_id"),
            Some("555".to_string())
        );
        assert_eq!(
            opt_string(&opts, "dbt_cloud.project_id"),
            Some("proj-999".to_string())
        );
    }
}
