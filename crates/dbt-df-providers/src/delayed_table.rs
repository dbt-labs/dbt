//! Table provider that defers data registration until execution time.

use datafusion::{
    datasource::listing::{ListingTable, ListingTableConfig, ListingTableUrl},
    execution::options::ReadOptions,
    prelude::{NdJsonReadOptions, ParquetReadOptions},
};
use datafusion_catalog::{Session, TableProvider};
use datafusion_common::error::DataFusionError;
use datafusion_expr::Expr;
use dbt_schema_store::{CanonicalFqn, DataStoreTrait, SchemaStoreTrait};
use std::any::Any;
use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;

use crate::seed_io::TableFormat;

// Ref to cache_state in delayed data table provider?

/// A [TableProvider] that allows the data source to be set after creation
///
/// This is used to allow storing the schema of a table/model independently from
/// the data: during the `Compile` phase, the `LogicalPlan`s for the models are
/// genereated, but the data is not yet available, so within those
/// `LogicalPlan`s the intermediate table/models must be represented by some
/// "schema only" [`TableProvider`]. Later, during the `Build` phase, the
/// `LogicalPlan` for each model is executed to produce the table data, which is
/// saved to some file. This presents a dilemma: since the downstream
/// `LogicalPlan`s are still referencing the "schema only" [`TableProvider`], how
/// would they be able to access the (newly generated) data for the upstream
/// tables? This is where the [DelayedDataTableProvider] comes in: during
/// `Compile` when the `LogicalPlan`s are constructed, a
/// [`DelayedDataTableProvider`] is instantiated with the schema for each model,
/// essentially acting as a "schema only" provider. Subsequently, during the
/// `Build` phase, the [`DelayedDataTableProvider`] is updated with the actual
/// data provider as each model gets executed, allowing the downstream
/// `LogicalPlan`s to access the data.
#[derive(Debug)]
pub struct DelayedDataTableProvider {
    // A unique name for this table, used for logging and error messages
    canonical_fqn: CanonicalFqn,
    // Ref to schema cache
    schema_store: Arc<dyn SchemaStoreTrait>,
    // Optional data store for locating persisted batches
    data_store: Arc<dyn DataStoreTrait>,
    // The data provider
    data_provider: OnceLock<Arc<dyn TableProvider>>,
}

impl DelayedDataTableProvider {
    /// Construct a new [`DelayedDataTableProvider`] backed by the schema store.
    pub fn new(
        canonical_fqn: CanonicalFqn,
        schema_cache: Arc<dyn SchemaStoreTrait>,
        data_store: Arc<dyn DataStoreTrait>,
    ) -> Self {
        Self {
            canonical_fqn,
            schema_store: schema_cache,
            data_store,
            data_provider: OnceLock::new(),
        }
    }
}

#[async_trait::async_trait]
impl TableProvider for DelayedDataTableProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> Arc<arrow_schema::Schema> {
        let schema_entry = self
            .schema_store
            .get_schema(&self.canonical_fqn)
            .unwrap_or_else(|| {
                panic!("Schema not found for canonical FQN: {}", self.canonical_fqn);
            });
        schema_entry.inner().clone()
    }

    /// Ensures the physical data provider exists before delegating to it.
    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn datafusion::physical_plan::ExecutionPlan>, DataFusionError> {
        // Set the data provider here...
        let schema = self.schema();
        let provider = if let Some(provider) = self.data_provider.get() {
            provider.clone()
        } else {
            let provider = make_listing_table_provider(
                state,
                &self.data_store.get_path_to_data(&self.canonical_fqn),
                TableFormat::Parquet,
            )
            .await?;
            let _ = self.data_provider.set(provider.clone());
            provider as Arc<dyn TableProvider>
        };
        let physical_schema = provider.schema();
        // TODO we cannot simply pass filters to source, if they use columns that 'needs_cast' below
        let base_plan = provider.scan(state, projection, filters, limit).await?;

        let projection: Vec<_> = (if let Some(projection) = projection {
            Box::new(projection.iter().cloned()) as Box<dyn Iterator<Item = usize>>
        } else {
            Box::new(0..schema.fields().len()) as Box<dyn Iterator<Item = usize>>
        })
        .enumerate()
        .map(|(base_plan_idx, schema_idx)| {
            let field = &physical_schema.fields()[schema_idx];
            let overlay_field = &schema.fields()[schema_idx];
            let needs_cast = field.data_type() != overlay_field.data_type();
            (
                if needs_cast {
                    // TODO this should use sdf_frontend::sql::common::maybe_cast_expr to create correct casting expression.
                    Arc::new(datafusion::physical_expr::expressions::CastExpr::new(
                        Arc::new(datafusion::physical_expr::expressions::Column::new(
                            field.name(),
                            base_plan_idx,
                        )),
                        overlay_field.data_type().clone(),
                        None,
                    )) as Arc<dyn datafusion::physical_expr::PhysicalExpr>
                } else {
                    Arc::new(datafusion::physical_expr::expressions::Column::new(
                        field.name(),
                        base_plan_idx,
                    )) as Arc<dyn datafusion::physical_expr::PhysicalExpr>
                },
                overlay_field.name().to_string(),
            )
        })
        .collect();

        Ok(Arc::new(
            datafusion::physical_plan::projection::ProjectionExec::try_new(projection, base_plan)?,
        ))
    }

    fn table_type(&self) -> datafusion::datasource::TableType {
        datafusion::datasource::TableType::Base
    }
}

/// Checks if the source schema can be safely overlaid with the target schema.
pub fn is_schema_compat(source: &arrow_schema::Schema, target: &arrow_schema::Schema) -> bool {
    source
        .fields()
        .iter()
        .zip(target.fields())
        .all(|(a, b)| a.name() == b.name())
        && source.fields().len() == target.fields().len()
}

/// Build a [`ListingTable`] over a single on-disk parquet/JSON file, inferring
/// the schema from the file. Used by [`DelayedDataTableProvider::scan`] to
/// lazily materialize a `TableProvider` over the upstream's persisted parquet
/// at scan time.
async fn make_listing_table_provider(
    ctx: &dyn Session,
    table_path: &Path,
    table_format: TableFormat,
) -> Result<Arc<ListingTable>, DataFusionError> {
    let listing_options = match table_format {
        TableFormat::Parquet => {
            ParquetReadOptions::new().to_listing_options(ctx.config(), ctx.table_options().clone())
        }
        TableFormat::Csv => {
            return Err(DataFusionError::Internal(
                "TableFormat::Csv is not supported in make_listing_table_provider".to_string(),
            ));
        }
        TableFormat::Json => NdJsonReadOptions::default()
            .to_listing_options(ctx.config(), ctx.table_options().clone()),
    };
    let (table_path, schema) =
        infer_schema_for_listing_options(ctx, table_path, &listing_options).await?;

    let config = ListingTableConfig::new(table_path)
        .with_listing_options(listing_options)
        .with_schema(schema);
    let provider = Arc::new(ListingTable::try_new(config).unwrap());
    Ok(provider)
}

async fn infer_schema_for_listing_options(
    ctx: &dyn Session,
    table_path: &Path,
    listing_options: &datafusion::datasource::listing::ListingOptions,
) -> Result<(ListingTableUrl, Arc<arrow_schema::Schema>), DataFusionError> {
    let table_url = ListingTableUrl::parse(table_path.to_string_lossy().as_ref())?;
    let schema = listing_options.infer_schema(ctx, &table_url).await?;
    Ok((table_url, schema))
}
