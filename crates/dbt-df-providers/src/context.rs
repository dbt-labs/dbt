//! Helpers for wiring DataFusion `SessionContext` to the schema store.

use datafusion::{
    execution::SessionStateBuilder,
    prelude::{SessionConfig, SessionContext},
};
use datafusion_common::Result;
use dbt_schema_store::{DataStoreTrait, SchemaStoreTrait};
use std::sync::Arc;

use crate::catalog_list::SchemaStoreCatalogProviderList;

/// Builds a [`SessionContext`] that uses the schema store for catalog lookups.
pub fn init_session_context(
    store: Arc<dyn SchemaStoreTrait>,
    data_store: Arc<dyn DataStoreTrait>,
) -> Result<SessionContext> {
    let session_config = get_default_session_config()?;
    init_session_context_from_config(session_config, store, data_store)
}

/// Returns the baseline [`SessionConfig`] used by Fusion.
///
/// The configuration disables DataFusion's default in-memory catalogs because
/// we provide our own schema store backed implementations.
pub fn get_default_session_config() -> Result<SessionConfig> {
    let mut session_config = SessionConfig::from_env()?
        // Note: we cannot allow DataFusion to create the default catalog
        // and schema, because it'll create them using the default DF
        // in-memory catalog/schema providers:
        .with_create_default_catalog_and_schema(false)
        .with_information_schema(false);
    // TODO see https://github.com/apache/datafusion/issues/12733
    session_config
        .options_mut()
        .execution
        .skip_physical_aggregate_schema_check = true;
    Ok(session_config)
}

/// Builds a [`SessionContext`] using the baseline config, optionally overriding
/// the execution timezone (sourced by dbt from the active profile's
/// `execution_timezone` setting). This is the one-call helper used by dbt-main
/// / dbt-lsp when they construct a `SessionComputeBackend` to inject into
/// `dbt-tasks::build_compiler_env`.
pub fn init_session_context_with_time_zone(
    execution_time_zone: Option<&str>,
    store: Arc<dyn SchemaStoreTrait>,
    data_store: Arc<dyn DataStoreTrait>,
) -> Result<SessionContext> {
    let mut config = get_default_session_config()?;
    if let Some(tz) = execution_time_zone {
        config.options_mut().execution.time_zone = tz.to_string();
    }
    init_session_context_from_config(config, store, data_store)
}

/// Builds a [`SessionContext`] from an explicit [`SessionConfig`], wiring the
/// schema store into the catalog list.
pub fn init_session_context_from_config(
    session_config: SessionConfig,
    store: Arc<dyn SchemaStoreTrait>,
    data_store: Arc<dyn DataStoreTrait>,
) -> Result<SessionContext> {
    let catalog_list = Arc::new(SchemaStoreCatalogProviderList::new(store, data_store));
    let state = SessionStateBuilder::new()
        .with_config(session_config)
        // .with_runtime_env(runtime_env)
        .with_catalog_list(catalog_list)
        .with_default_features()
        .with_analyzer_rules(vec![])
        .build();
    let ctx = SessionContext::new_with_state(state);
    Ok(ctx)
}
