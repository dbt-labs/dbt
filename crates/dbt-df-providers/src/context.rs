//! Helpers for wiring DataFusion `SessionContext` to the schema store.

use datafusion::{
    execution::SessionStateBuilder,
    logical_expr::LogicalPlan,
    optimizer::{
        ApplyOrder, Optimizer, OptimizerConfig, OptimizerRule,
        simplify_expressions::SimplifyExpressions,
    },
    prelude::{SessionConfig, SessionContext},
};
use datafusion_common::{Result, tree_node::Transformed};
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

/// Execution time zone used when none is configured. DataFusion 53 leaves
/// `execution.time_zone` unset by default; it used to be `"+00:00"`.
pub const DEFAULT_EXECUTION_TIME_ZONE: &str = "+00:00";

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
    session_config
        .options_mut()
        .execution
        .time_zone
        .get_or_insert_with(|| DEFAULT_EXECUTION_TIME_ZONE.to_string());
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
        config.options_mut().execution.time_zone = Some(tz.to_string());
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
        .with_optimizer_rules(optimizer_rules())
        .build();
    let ctx = SessionContext::new_with_state(state);
    Ok(ctx)
}

/// DataFusion's default optimizer rules, with `simplify_expressions` replaced
/// by [`LenientSimplifyExpressions`].
pub fn optimizer_rules() -> Vec<Arc<dyn OptimizerRule + Send + Sync>> {
    Optimizer::new()
        .rules
        .into_iter()
        .map(|rule| -> Arc<dyn OptimizerRule + Send + Sync> {
            if rule.name() == SimplifyExpressions::new().name() {
                Arc::new(LenientSimplifyExpressions::default())
            } else {
                rule
            }
        })
        .collect()
}

/// [`SimplifyExpressions`], except that a plan node whose expressions fail to
/// simplify is left as is, and execution reports any error.
///
/// Since DataFusion 53, the simplifier fails planning on a CAST of a literal
/// that Arrow cannot parse (e.g. '1970/03/15' or 'infinity' as a timestamp),
/// even though the target warehouse may accept it.
#[derive(Debug, Default)]
pub struct LenientSimplifyExpressions(SimplifyExpressions);

impl OptimizerRule for LenientSimplifyExpressions {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn apply_order(&self) -> Option<ApplyOrder> {
        self.0.apply_order()
    }

    fn rewrite(
        &self,
        plan: LogicalPlan,
        config: &dyn OptimizerConfig,
    ) -> Result<Transformed<LogicalPlan>> {
        // The rule is applied node by node, so this clones a single node.
        match self.0.rewrite(plan.clone(), config) {
            Ok(transformed) => Ok(transformed),
            Err(_) => Ok(Transformed::no(plan)),
        }
    }
}
