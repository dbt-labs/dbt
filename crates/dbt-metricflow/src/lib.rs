//! MetricFlow semantic query compiler — translates metric queries into executable SQL.
//!
//! This crate provides a storage-agnostic compiler: callers implement the
//! [`MetricStore`] trait to supply metric/model metadata, and the compiler
//! produces SQL for the requested dialect.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::str::FromStr;

// ═══════════════════════════════════════════════════════════════════════════
// Error type
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, thiserror::Error)]
pub enum MetricFlowError {
    #[error("{0}")]
    Other(String),
}

// ═══════════════════════════════════════════════════════════════════════════
// Metadata store trait
// ═══════════════════════════════════════════════════════════════════════════

/// Raw metric row from the metadata store.
#[derive(Debug, Clone)]
pub struct RawMetricRow {
    pub name: String,
    pub metric_type: String,
    pub description: String,
    pub type_params: String,
    pub metric_filter: String,
    pub time_granularity: Option<String>,
}

/// Raw semantic model row.
#[derive(Debug, Clone, Default)]
pub struct RawModelRow {
    pub name: String,
    pub node_relation: String,
    pub primary_entity: String,
    pub unique_id: String,
    /// SCD validity window columns (is_valid_from, is_valid_to).
    pub scd_valid_from: Option<String>,
    pub scd_valid_to: Option<String>,
}

/// Raw entity row.
#[derive(Debug, Clone)]
pub struct RawEntityRow {
    pub name: String,
    pub entity_type: String,
    pub expr: String,
}

/// Raw dimension row.
#[derive(Debug, Clone, Default)]
pub struct RawDimensionRow {
    pub name: String,
    pub dimension_type: String,
    pub expr: String,
    pub time_granularity: String,
    pub is_partition: bool,
}

/// Raw join graph entry.
#[derive(Debug, Clone)]
pub struct RawJoinGraphRow {
    pub model_name: String,
    pub entity_name: String,
    pub entity_type: String,
    pub expr: String,
}

/// Raw time spine row.
#[derive(Debug, Clone)]
pub struct RawTimeSpineRow {
    pub node_relation: String,
    pub primary_column: String,
    pub primary_granularity: String,
    pub custom_granularities: Vec<(String, String)>,
}

/// Abstraction over the metadata store (dbt index, or any other source).
pub trait MetricStore {
    fn lookup_metric(&mut self, name: &str) -> Result<Option<RawMetricRow>, MetricFlowError>;
    fn list_metric_names(&mut self) -> Result<Vec<String>, MetricFlowError>;
    fn lookup_semantic_model(&mut self, name: &str)
    -> Result<Option<RawModelRow>, MetricFlowError>;
    fn lookup_model_entities(
        &mut self,
        unique_id: &str,
    ) -> Result<Vec<RawEntityRow>, MetricFlowError>;
    fn lookup_model_dimensions(
        &mut self,
        unique_id: &str,
    ) -> Result<Vec<RawDimensionRow>, MetricFlowError>;
    fn lookup_all_join_graph_entities(&mut self) -> Result<Vec<RawJoinGraphRow>, MetricFlowError>;
    fn find_model_for_entity(
        &mut self,
        entity_name: &str,
        primary_or_unique_only: bool,
    ) -> Result<Option<String>, MetricFlowError>;
    fn check_entity_in_model(
        &mut self,
        model_name: &str,
        entity_name: &str,
    ) -> Result<bool, MetricFlowError>;
    fn lookup_time_spine(&mut self) -> Result<Option<RawTimeSpineRow>, MetricFlowError>;
    fn lookup_all_time_spines(&mut self) -> Result<Vec<RawTimeSpineRow>, MetricFlowError> {
        Ok(self.lookup_time_spine()?.into_iter().collect())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// In-memory metric store (for testing and embedding)
// ═══════════════════════════════════════════════════════════════════════════

/// A [`MetricStore`] backed by in-memory data structures.
///
/// Build one from a semantic manifest JSON blob via [`InMemoryMetricStore::from_manifest`],
/// or populate it manually for tests.
#[derive(Debug, Default)]
pub struct InMemoryMetricStore {
    pub metrics: HashMap<String, RawMetricRow>,
    pub models: HashMap<String, RawModelRow>,
    /// Insertion-order model names (for deterministic iteration).
    pub model_order: Vec<String>,
    pub entities: HashMap<String, Vec<RawEntityRow>>,
    pub dimensions: HashMap<String, Vec<RawDimensionRow>>,
    pub time_spine: Option<RawTimeSpineRow>,
    pub time_spines: Vec<RawTimeSpineRow>,
}

impl InMemoryMetricStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build from a semantic manifest JSON value (the format produced by dbt).
    pub fn from_manifest(manifest: &serde_json::Value) -> Self {
        let mut store = Self::new();

        if let Some(models) = manifest.get("semantic_models").and_then(|v| v.as_array()) {
            for m in models {
                let name = m.get("name").and_then(|v| v.as_str()).unwrap_or_default();
                let nr = m
                    .get("node_relation")
                    .map(|v| v.to_string())
                    .unwrap_or_default();
                let pe = m
                    .get("primary_entity")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let unique_id = format!("semantic_model.test.{name}");

                // Parse optional SCD validity_params.
                let vp = m.get("validity_params");
                let scd_valid_from = vp
                    .and_then(|v| v.get("is_valid_from"))
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let scd_valid_to = vp
                    .and_then(|v| v.get("is_valid_to"))
                    .and_then(|v| v.as_str())
                    .map(String::from);

                store.model_order.push(name.to_string());
                store.models.insert(
                    name.to_string(),
                    RawModelRow {
                        name: name.to_string(),
                        node_relation: nr,
                        primary_entity: pe.to_string(),
                        unique_id: unique_id.clone(),
                        scd_valid_from,
                        scd_valid_to,
                    },
                );

                // Entities
                let mut ents = Vec::new();
                if let Some(arr) = m.get("entities").and_then(|v| v.as_array()) {
                    for e in arr {
                        ents.push(RawEntityRow {
                            name: e
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            entity_type: e
                                .get("type")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            expr: e
                                .get("expr")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                        });
                    }
                }
                // Include primary_entity as a synthetic entity if not already
                // in the explicit list — needed for dimension-only query
                // resolution where entities drive model lookup.
                if !pe.is_empty() && !ents.iter().any(|e| e.name == pe) {
                    ents.push(RawEntityRow {
                        name: pe.to_string(),
                        entity_type: "primary".to_string(),
                        expr: pe.to_string(),
                    });
                }

                store.entities.insert(unique_id.clone(), ents);

                // Dimensions
                let mut dims = Vec::new();
                if let Some(arr) = m.get("dimensions").and_then(|v| v.as_array()) {
                    for d in arr {
                        let gran = d
                            .get("type_params")
                            .and_then(|tp| tp.get("time_granularity"))
                            .and_then(|v| v.as_str())
                            .unwrap_or_default();
                        dims.push(RawDimensionRow {
                            name: d
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            dimension_type: d
                                .get("type")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            expr: d
                                .get("expr")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            time_granularity: gran.to_string(),
                            is_partition: d
                                .get("is_partition")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false),
                        });
                    }
                }
                store.dimensions.insert(unique_id, dims);
            }
        }

        if let Some(metrics) = manifest.get("metrics").and_then(|v| v.as_array()) {
            for m in metrics {
                let name = m.get("name").and_then(|v| v.as_str()).unwrap_or_default();
                store.metrics.insert(
                    name.to_string(),
                    RawMetricRow {
                        name: name.to_string(),
                        metric_type: m
                            .get("type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("simple")
                            .to_string(),
                        description: m
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        type_params: m
                            .get("type_params")
                            .map(|v| v.to_string())
                            .unwrap_or_default(),
                        metric_filter: m.get("filter").map(|v| v.to_string()).unwrap_or_default(),
                        time_granularity: m
                            .get("time_granularity")
                            .and_then(|v| v.as_str())
                            .map(String::from),
                    },
                );
            }
        }

        if let Some(spines) = manifest.get("project_configuration").and_then(|v| {
            v.get("time_spines")
                .or_else(|| v.get("time_spine_table_configurations"))
                .and_then(|v| v.as_array())
        }) {
            for s in spines {
                let nr = s
                    .get("node_relation")
                    .map(|v| v.to_string())
                    .unwrap_or_default();
                let col = s
                    .get("primary_column")
                    .and_then(|v| v.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("ds");
                let gran = s
                    .get("primary_column")
                    .and_then(|v| v.get("time_granularity"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("day");
                let custom_grans: Vec<(String, String)> = s
                    .get("custom_granularities")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|cg| {
                                let name = cg.get("name")?.as_str()?;
                                let column = cg
                                    .get("column_name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(name);
                                Some((name.to_string(), column.to_string()))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let row = RawTimeSpineRow {
                    node_relation: nr,
                    primary_column: col.to_string(),
                    primary_granularity: gran.to_string(),
                    custom_granularities: custom_grans,
                };
                if store.time_spine.is_none() {
                    store.time_spine = Some(row.clone());
                }
                store.time_spines.push(row);
            }
        }

        store
    }
}

impl MetricStore for InMemoryMetricStore {
    fn lookup_metric(&mut self, name: &str) -> Result<Option<RawMetricRow>, MetricFlowError> {
        Ok(self.metrics.get(name).cloned())
    }

    fn list_metric_names(&mut self) -> Result<Vec<String>, MetricFlowError> {
        let mut names: Vec<String> = self.metrics.keys().cloned().collect();
        names.sort();
        Ok(names)
    }

    fn lookup_semantic_model(
        &mut self,
        name: &str,
    ) -> Result<Option<RawModelRow>, MetricFlowError> {
        Ok(self.models.get(name).cloned())
    }

    fn lookup_model_entities(
        &mut self,
        unique_id: &str,
    ) -> Result<Vec<RawEntityRow>, MetricFlowError> {
        Ok(self.entities.get(unique_id).cloned().unwrap_or_default())
    }

    fn lookup_model_dimensions(
        &mut self,
        unique_id: &str,
    ) -> Result<Vec<RawDimensionRow>, MetricFlowError> {
        Ok(self.dimensions.get(unique_id).cloned().unwrap_or_default())
    }

    fn lookup_all_join_graph_entities(&mut self) -> Result<Vec<RawJoinGraphRow>, MetricFlowError> {
        let mut rows = Vec::new();
        for model_name in &self.model_order {
            if let Some(model) = self.models.get(model_name) {
                if let Some(ents) = self.entities.get(&model.unique_id) {
                    for e in ents {
                        rows.push(RawJoinGraphRow {
                            model_name: model_name.clone(),
                            entity_name: e.name.clone(),
                            entity_type: e.entity_type.clone(),
                            expr: e.expr.clone(),
                        });
                    }
                }
            }
        }
        Ok(rows)
    }

    fn find_model_for_entity(
        &mut self,
        entity_name: &str,
        primary_or_unique_only: bool,
    ) -> Result<Option<String>, MetricFlowError> {
        for model_name in &self.model_order {
            if let Some(model) = self.models.get(model_name) {
                if let Some(ents) = self.entities.get(&model.unique_id) {
                    for e in ents {
                        if e.name == entity_name {
                            if primary_or_unique_only
                                && !matches!(
                                    e.entity_type.as_str(),
                                    "primary" | "unique" | "natural"
                                )
                            {
                                continue;
                            }
                            return Ok(Some(model_name.clone()));
                        }
                    }
                }
            }
        }
        Ok(None)
    }

    fn check_entity_in_model(
        &mut self,
        model_name: &str,
        entity_name: &str,
    ) -> Result<bool, MetricFlowError> {
        if let Some(model) = self.models.get(model_name) {
            if let Some(ents) = self.entities.get(&model.unique_id) {
                return Ok(ents.iter().any(|e| e.name == entity_name));
            }
        }
        Ok(false)
    }

    fn lookup_time_spine(&mut self) -> Result<Option<RawTimeSpineRow>, MetricFlowError> {
        Ok(self.time_spine.clone())
    }

    fn lookup_all_time_spines(&mut self) -> Result<Vec<RawTimeSpineRow>, MetricFlowError> {
        Ok(self.time_spines.clone())
    }
}

/// SQL dialect for rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    DuckDB,
    Snowflake,
    Redshift,
    BigQuery,
    Databricks,
}

impl FromStr for Dialect {
    type Err = MetricFlowError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "duckdb" | "duck" => Ok(Dialect::DuckDB),
            "snowflake" | "sf" => Ok(Dialect::Snowflake),
            "redshift" | "rs" => Ok(Dialect::Redshift),
            "bigquery" | "bq" => Ok(Dialect::BigQuery),
            "databricks" | "spark" => Ok(Dialect::Databricks),
            other => Err(MetricFlowError::Other(format!(
                "unknown dialect: {other:?}. Use 'duckdb', 'snowflake', 'redshift', 'bigquery', or 'databricks'"
            ))),
        }
    }
}

/// A parsed semantic query specification.
#[derive(Debug, Clone)]
pub struct SemanticQuerySpec {
    pub metrics: Vec<String>,
    pub group_by: Vec<GroupBySpec>,
    pub where_filters: Vec<String>,
    pub order_by: Vec<OrderBySpec>,
    pub limit: Option<usize>,
    /// Optional time constraint: `[start_date, end_date]` applied as
    /// `WHERE metric_time >= start AND metric_time <= end`.
    pub time_constraint: Option<(String, String)>,
    /// When false, skip GROUP BY (produce all rows without deduplication).
    pub apply_group_by: bool,
}

/// A group-by specification — either a dimension or time dimension.
#[derive(Debug, Clone)]
pub enum GroupBySpec {
    Dimension {
        entity: Option<String>,
        name: String,
    },
    TimeDimension {
        name: String,
        granularity: String,
        /// When set, extract a date part instead of truncating (e.g., EXTRACT(YEAR FROM ...)).
        date_part: Option<String>,
    },
    /// Group by an entity column (e.g., `Entity('listing')` → `listing_id`).
    Entity { name: String },
}

/// Compute the output column alias for each group-by spec.
///
/// When a single time dimension name appears at multiple granularities
/// (e.g. `metric_time__month` + `metric_time__week`), the alias is
/// disambiguated as `name__granularity`. Otherwise the bare `name` is used.
fn group_by_output_cols(group_by: &[GroupBySpec]) -> Vec<String> {
    let mut time_dim_counts: HashMap<&str, usize> = HashMap::new();
    for gb in group_by {
        if let GroupBySpec::TimeDimension { name, .. } = gb {
            *time_dim_counts.entry(name.as_str()).or_default() += 1;
        }
    }
    group_by
        .iter()
        .map(|gb| match gb {
            GroupBySpec::TimeDimension {
                name,
                granularity,
                date_part,
            } => {
                if let Some(part) = date_part {
                    format!("{name}__extract_{part}")
                } else if time_dim_counts.get(name.as_str()).copied().unwrap_or(1) > 1
                    || !is_standard_granularity(granularity)
                {
                    format!("{name}__{granularity}")
                } else {
                    name.clone()
                }
            }
            GroupBySpec::Dimension {
                entity: Some(e),
                name,
            } => format!("{e}__{name}"),
            GroupBySpec::Dimension { entity: None, name } => name.clone(),
            GroupBySpec::Entity { name } => name.clone(),
        })
        .collect()
}

/// An order-by specification.
#[derive(Debug, Clone)]
pub struct OrderBySpec {
    pub name: String,
    pub descending: bool,
}

/// Metric type as stored in dbt.metrics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetricType {
    Simple,
    Derived,
    Ratio,
    Cumulative,
    Conversion,
}

/// A resolved metric with all information needed for compilation.
#[derive(Debug, Clone)]
pub struct ResolvedMetric {
    pub name: String,
    pub metric_type: MetricType,
    pub description: String,
    /// For simple metrics: the aggregation details.
    pub agg_params: Option<AggParams>,
    /// For simple metrics: where filters from the metric definition.
    pub metric_filters: Vec<String>,
    /// For derived metrics: the expression template.
    pub derived_expr: Option<String>,
    /// For derived/ratio metrics: input metric names.
    pub input_metrics: Vec<MetricInput>,
    /// For ratio metrics: numerator and denominator.
    pub numerator: Option<MetricInput>,
    pub denominator: Option<MetricInput>,
    /// For cumulative metrics.
    pub cumulative_params: Option<CumulativeParams>,
    /// For conversion metrics.
    pub conversion_params: Option<ConversionParams>,
    /// Whether to join to time spine for null-filling.
    pub join_to_timespine: bool,
    /// Value to fill nulls with.
    pub fill_nulls_with: Option<i64>,
    /// Metric-level time granularity (e.g., "month" for a monthly-grain metric).
    pub time_granularity: Option<String>,
}

/// Aggregation parameters for a simple metric's measure.
#[derive(Debug, Clone)]
pub struct AggParams {
    pub semantic_model: String,
    pub agg: String,
    pub expr: String,
    pub agg_time_dimension: Option<String>,
    pub non_additive_dimension: Option<serde_json::Value>,
    pub percentile: Option<f64>,
    pub use_discrete_percentile: bool,
}

/// A reference to an input metric (used in derived/ratio metrics).
#[derive(Debug, Clone)]
pub struct MetricInput {
    pub name: String,
    pub filters: Vec<String>,
    pub alias: Option<String>,
    pub offset_window: Option<String>,
    pub offset_to_grain: Option<String>,
}

/// Cumulative metric parameters.
#[derive(Debug, Clone)]
pub struct CumulativeParams {
    pub window_count: Option<i64>,
    pub window_granularity: Option<String>,
    pub grain_to_date: Option<String>,
}

/// Conversion metric parameters.
#[derive(Debug, Clone)]
pub struct ConversionParams {
    pub entity: String,
    pub base_metric: String,
    pub conversion_metric: String,
    pub calculation: String,
    pub window_count: Option<i64>,
    pub window_granularity: Option<String>,
    pub constant_properties: Vec<(String, String)>,
}

/// A resolved semantic model with its physical table and join keys.
#[derive(Debug, Clone)]
pub struct ResolvedModel {
    pub name: String,
    pub relation_name: String,
    pub alias: String,
    pub schema_name: String,
    pub database: String,
    pub primary_entity: Option<String>,
    pub entities: Vec<EntityDef>,
    pub dimensions: Vec<DimensionDef>,
    /// SCD validity window columns, if this is a slowly changing dimension.
    pub scd_valid_from: Option<String>,
    pub scd_valid_to: Option<String>,
}

/// Entity definition within a semantic model.
#[derive(Debug, Clone)]
pub struct EntityDef {
    pub name: String,
    pub entity_type: String,
    pub expr: String,
}

/// Dimension definition within a semantic model.
#[derive(Debug, Clone, Default)]
pub struct DimensionDef {
    pub name: String,
    pub dimension_type: String,
    pub expr: String,
    pub time_granularity: Option<String>,
    pub is_partition: bool,
}

/// Time spine information.
#[derive(Debug, Clone)]
pub struct TimeSpine {
    pub relation_name: String,
    pub primary_column: String,
    pub primary_granularity: String,
    pub custom_granularities: Vec<(String, String)>,
}

/// A join between two semantic models via a shared entity.
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct JoinEdge {
    from_model: String,
    to_model: String,
    from_expr: String,
    to_expr: String,
    entity_name: String,
}

/// A pre-computed multi-hop subquery join.
/// For entity chains like `account_id__customer_id__customer_name`, we need a
/// subquery that chains the intermediate tables independently (each dimension
/// from a different leaf model gets its own copy of the bridge).
#[derive(Debug)]
struct MultiHopSubquery {
    /// Alias for the subquery (e.g., `__mh0`).
    alias: String,
    /// Full subquery SQL (without the alias).
    subquery_sql: String,
    /// The expression on the fact/primary side of the join (e.g., `a.account_id`).
    fact_join_expr: String,
    /// The column name in the subquery that joins to the fact table.
    subquery_join_col: String,
    /// Map from dimension name → column expression in the subquery.
    dim_columns: HashMap<String, String>,
}

// ═══════════════════════════════════════════════════════════════════════════
// Input validation
// ═══════════════════════════════════════════════════════════════════════════

/// Check that a string is a valid SQL identifier (alphanumeric + underscore).
fn is_valid_identifier(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Check if `haystack` contains `word` as a whole word (surrounded by non-alphanumeric or boundaries).
fn contains_word(haystack: &str, word: &str) -> bool {
    let bytes = haystack.as_bytes();
    let wlen = word.len();
    for (i, _) in haystack.match_indices(word) {
        let before_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        let after_ok = i + wlen >= bytes.len() || !bytes[i + wlen].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
    }
    false
}

/// Reject WHERE filter strings that contain dangerous SQL patterns.
/// Allowed: `{{ Dimension/TimeDimension/Metric(...) }}` templates, comparison operators,
/// literals, AND/OR/NOT, IS NULL, IN (...), BETWEEN, LIKE.
/// Rejected: semicolons, comments, subqueries, DDL/DML keywords, UNION.
fn validate_where_filter(filter: &str) -> Result<(), MetricFlowError> {
    // Strip out recognized Jinja templates so they don't trigger false positives.
    let mut stripped = filter.to_string();
    while let Some(start) = stripped.find("{{") {
        if let Some(end) = stripped[start..].find("}}").map(|i| start + i + 2) {
            stripped.replace_range(start..end, "TMPL");
        } else {
            break;
        }
    }

    let upper = stripped.to_ascii_uppercase();

    // Reject semicolons (statement chaining).
    if stripped.contains(';') {
        return Err(MetricFlowError::Other(
            "WHERE filter must not contain ';'".into(),
        ));
    }

    // Reject SQL comments.
    if stripped.contains("--") || stripped.contains("/*") {
        return Err(MetricFlowError::Other(
            "WHERE filter must not contain SQL comments".into(),
        ));
    }

    // Reject dangerous keywords (word-boundary matched).
    static DANGEROUS: &[&str] = &[
        "DROP", "DELETE", "INSERT", "UPDATE", "ALTER", "CREATE", "EXEC", "EXECUTE", "UNION", "INTO",
    ];
    for kw in DANGEROUS {
        if contains_word(&upper, kw) {
            return Err(MetricFlowError::Other(format!(
                "WHERE filter must not contain '{kw}'"
            )));
        }
    }

    Ok(())
}

/// Valid time granularities supported by MetricFlow / DuckDB / Snowflake.
pub const VALID_GRANULARITIES: &[&str] = &[
    "day",
    "week",
    "month",
    "quarter",
    "year",
    "hour",
    "minute",
    "second",
    "millisecond",
];

/// Validate Dimension/TimeDimension references inside WHERE filter strings.
fn validate_where_dim_refs(
    filters: &[String],
    avail_dims: &[String],
    avail_time_dims: &[String],
) -> Result<(), MetricFlowError> {
    let all_dim_names: Vec<&str> = avail_dims
        .iter()
        .chain(avail_time_dims.iter())
        .map(|s| s.as_str())
        .collect();

    for filter in filters {
        // Extract Dimension('...') references (but not TimeDimension).
        let mut cursor = 0usize;
        while let Some(start) = filter[cursor..].find("Dimension('") {
            let abs_pos = cursor + start;
            if abs_pos > 0 && filter.as_bytes()[abs_pos - 1].is_ascii_alphanumeric() {
                cursor = abs_pos + 11;
                continue;
            }
            let abs_start = abs_pos + 11; // skip "Dimension('"
            if let Some(end) = filter[abs_start..].find("')") {
                let dim_ref = &filter[abs_start..abs_start + end];
                let base_name = dim_ref.split("__").last().unwrap_or(dim_ref);
                let found = all_dim_names
                    .iter()
                    .any(|d| *d == dim_ref || d.ends_with(&format!("__{base_name}")));
                if !found {
                    return Err(MetricFlowError::Other(format!(
                        "unknown dimension in where filter: {dim_ref:?}\n\
                         Available dimensions: {}\n\
                         Available time dimensions: {}",
                        avail_dims.join(", "),
                        avail_time_dims.join(", ")
                    )));
                }
                cursor = abs_start + end + 2;
            } else {
                break;
            }
        }
        // Extract TimeDimension('...', '...') references.
        cursor = 0;
        while let Some(start) = filter[cursor..].find("TimeDimension(") {
            let abs_start = cursor + start + 14; // skip "TimeDimension("
            if let Some(end) = filter[abs_start..].find(')') {
                let inner = &filter[abs_start..abs_start + end];
                let args: Vec<&str> = inner.split(',').collect();
                let dim_name = args
                    .first()
                    .map(|a| a.trim().trim_matches('\'').trim_matches('"'))
                    .unwrap_or("");
                let base_name = dim_name.split("__").last().unwrap_or(dim_name);
                let found = avail_time_dims
                    .iter()
                    .any(|d| d == dim_name || d.ends_with(&format!("__{base_name}")));
                if !found {
                    return Err(MetricFlowError::Other(format!(
                        "unknown time dimension in where filter: {dim_name:?}\n\
                         Available time dimensions: {}",
                        avail_time_dims.join(", ")
                    )));
                }
                // Skip granularity validation — custom granularities are validated during compilation.
                cursor = abs_start + end + 1;
            } else {
                break;
            }
        }
    }
    Ok(())
}

/// Validate a query spec against the resolved semantic models.
///
/// Called from `compile()` after models are resolved — at that point we know
/// every dimension, entity, and metric available.  Returns a clear error
/// message listing the available options whenever a name doesn't match.
fn validate_spec(
    spec: &SemanticQuerySpec,
    all_metrics: &HashMap<String, ResolvedMetric>,
    model_aliases: &HashMap<String, (String, &ResolvedModel)>,
) -> Result<(), MetricFlowError> {
    // Collect all available dimension names (including entity-prefixed forms).
    let mut avail_dims: Vec<String> = Vec::new();
    let mut avail_time_dims: Vec<String> = Vec::new();
    let mut avail_entities: Vec<String> = Vec::new();

    for (_alias, model) in model_aliases.values() {
        // Collect all entity names for this model (explicit + primary).
        let mut model_entity_names: Vec<&str> =
            model.entities.iter().map(|e| e.name.as_str()).collect();
        if let Some(ref pe) = model.primary_entity {
            if !model_entity_names.contains(&pe.as_str()) {
                model_entity_names.push(pe.as_str());
            }
        }

        for dim in &model.dimensions {
            if dim.dimension_type == "time" {
                if !avail_time_dims.contains(&dim.name) {
                    avail_time_dims.push(dim.name.clone());
                }
            } else if !avail_dims.contains(&dim.name) {
                avail_dims.push(dim.name.clone());
            }
            // Also add entity-prefixed forms.
            for ent_name in &model_entity_names {
                let ref_form = format!("{ent_name}__{}", dim.name);
                if dim.dimension_type == "time" {
                    if !avail_time_dims.contains(&ref_form) {
                        avail_time_dims.push(ref_form);
                    }
                } else if !avail_dims.contains(&ref_form) {
                    avail_dims.push(ref_form);
                }
            }
        }
        for ent_name in &model_entity_names {
            let name_str = (*ent_name).to_string();
            if !avail_entities.contains(&name_str) {
                avail_entities.push(name_str);
            }
        }
    }
    // metric_time is always available as a synthetic time dimension.
    if !avail_time_dims.contains(&"metric_time".to_string()) {
        avail_time_dims.push("metric_time".to_string());
    }

    avail_dims.sort();
    avail_time_dims.sort();
    avail_entities.sort();

    // ── Validate group-by ──────────────────────────────────────────────
    for gb in &spec.group_by {
        match gb {
            GroupBySpec::TimeDimension { name, .. } => {
                // Check time dimension name.
                let base_name = name.split("__").last().unwrap_or(name);
                let found = avail_time_dims
                    .iter()
                    .any(|d| d == name || d.ends_with(&format!("__{base_name}")));
                if !found {
                    return Err(MetricFlowError::Other(format!(
                        "unknown time dimension: {name:?}\n\
                         Available time dimensions: {}",
                        avail_time_dims.join(", ")
                    )));
                }
            }
            GroupBySpec::Dimension { entity, name } => {
                let full_ref = match entity {
                    Some(e) => format!("{e}__{name}"),
                    None => name.clone(),
                };
                // For multi-hop paths (e.g. account_id__customer_id), use the
                // last entity in the chain for model lookup.
                let target_entity = entity
                    .as_ref()
                    .map(|e| e.rsplit_once("__").map_or(e.as_str(), |(_, last)| last));
                // Check that dimension exists in at least one resolved model.
                let found = model_aliases.values().any(|(_alias, model)| {
                    let has_entity_match = match target_entity {
                        Some(e) => {
                            model.entities.iter().any(|ent| ent.name == e)
                                || model.primary_entity.as_deref() == Some(e)
                        }
                        None => true,
                    };
                    if !has_entity_match {
                        return false;
                    }
                    // Check dimensions.
                    let in_dims = model
                        .dimensions
                        .iter()
                        .any(|d| d.dimension_type != "time" && d.name == *name);
                    // Also treat entities as selectable "dimensions" (dundered identifier).
                    let in_entities = model.entities.iter().any(|e| e.name == *name);
                    in_dims || in_entities
                });
                if !found {
                    return Err(MetricFlowError::Other(format!(
                        "unknown dimension: {full_ref:?}\n\
                         Available dimensions: {}\n\
                         Available time dimensions (use metric_time:day syntax): {}",
                        avail_dims.join(", "),
                        avail_time_dims.join(", ")
                    )));
                }
            }
            GroupBySpec::Entity { name } => {
                let found = model_aliases.values().any(|(_alias, model)| {
                    model.entities.iter().any(|e| e.name == *name)
                        || model.primary_entity.as_deref() == Some(name.as_str())
                });
                if !found {
                    return Err(MetricFlowError::Other(format!(
                        "unknown entity: {name:?}\n\
                         Available entities: {}",
                        avail_entities.join(", ")
                    )));
                }
            }
        }
    }

    // ── Validate order-by ──────────────────────────────────────────────
    // Order-by names must refer to a metric or a group-by dimension/entity.
    let metric_names: Vec<&str> = all_metrics.keys().map(|s| s.as_str()).collect();
    let group_by_names: Vec<String> = spec
        .group_by
        .iter()
        .flat_map(|gb| match gb {
            GroupBySpec::TimeDimension {
                name, granularity, ..
            } => {
                vec![name.clone(), format!("{name}__{granularity}")]
            }
            GroupBySpec::Dimension { entity, name } => match entity {
                Some(e) => vec![format!("{e}__{name}"), name.clone()],
                None => vec![name.clone()],
            },
            GroupBySpec::Entity { name } => vec![name.clone()],
        })
        .collect();

    for ob in &spec.order_by {
        let found = metric_names.contains(&ob.name.as_str())
            || group_by_names.iter().any(|g| g == &ob.name);
        if !found {
            let mut available: Vec<String> =
                metric_names.iter().map(|s| (*s).to_string()).collect();
            available.extend(group_by_names.iter().cloned());
            available.sort();
            available.dedup();
            return Err(MetricFlowError::Other(format!(
                "unknown order-by: {:?}\n\
                 Order-by must reference a metric or group-by column.\n\
                 Available: {}",
                ob.name,
                available.join(", ")
            )));
        }
    }

    // ── Validate where-filter dimension references ─────────────────────
    validate_where_dim_refs(&spec.where_filters, &avail_dims, &avail_time_dims)?;

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Query spec parsing
// ═══════════════════════════════════════════════════════════════════════════

/// Parse a JSON semantic query specification.
///
/// Expected format:
/// ```json
/// {
///   "metrics": ["revenue", "order_count"],
///   "group_by": ["TimeDimension('metric_time', 'day')", "Dimension('customer__segment')"],
///   "where": ["{{ Dimension('order_id__status') }} = 'completed'"],
///   "order_by": ["-revenue", "+metric_time"],
///   "limit": 100
/// }
/// ```
pub fn parse_query_spec(json_str: &str) -> Result<SemanticQuerySpec, MetricFlowError> {
    let v: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| MetricFlowError::Other(format!("invalid query JSON: {e}")))?;

    let metrics = v
        .get("metrics")
        .and_then(|m| m.as_array())
        .ok_or_else(|| MetricFlowError::Other("query must have a 'metrics' array".into()))?
        .iter()
        .filter_map(|m| m.as_str().map(String::from))
        .collect::<Vec<_>>();

    // Dimension-only queries (no metrics) are allowed — they select only
    // dimensions/entities from semantic models.

    let group_by = match v.get("group_by").and_then(|g| g.as_array()) {
        Some(arr) => {
            let mut parsed = Vec::with_capacity(arr.len());
            for item in arr {
                let s = item.as_str().ok_or_else(|| {
                    MetricFlowError::Other("group_by items must be strings".into())
                })?;
                match parse_group_by_str(s) {
                    Some(gb) => parsed.push(gb),
                    None => {
                        return Err(MetricFlowError::Other(format!(
                            "invalid group_by: {s:?}\n\
                             Expected formats:\n  \
                               metric_time:day          (time dimension with granularity)\n  \
                               customer__segment        (entity-prefixed dimension)\n  \
                               segment                  (plain dimension)\n  \
                               TimeDimension('metric_time', 'day')\n  \
                               Dimension('customer__segment')\n  \
                               Entity('listing')"
                        )));
                    }
                }
            }
            parsed
        }
        None => vec![],
    };

    let where_filters: Vec<String> = v
        .get("where")
        .and_then(|w| w.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|w| w.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    // Validate WHERE filters against SQL injection patterns.
    for filter in &where_filters {
        validate_where_filter(filter)?;
    }

    let order_by = match v.get("order_by").and_then(|o| o.as_array()) {
        Some(arr) => {
            let mut parsed = Vec::with_capacity(arr.len());
            for item in arr {
                let s = item.as_str().ok_or_else(|| {
                    MetricFlowError::Other("order_by items must be strings".into())
                })?;
                match parse_order_by_str(s) {
                    Some(ob) => parsed.push(ob),
                    None => {
                        return Err(MetricFlowError::Other(format!(
                            "invalid order_by: {s:?}\n\
                             Expected formats:\n  \
                               revenue           (ascending)\n  \
                               -revenue          (descending)\n  \
                               +metric_time      (ascending, explicit)"
                        )));
                    }
                }
            }
            parsed
        }
        None => vec![],
    };

    let limit = v.get("limit").and_then(|l| l.as_u64()).map(|l| l as usize);

    let time_constraint = v
        .get("time_constraint")
        .and_then(|tc| tc.as_array())
        .and_then(|arr| {
            let start = arr.first()?.as_str()?.to_string();
            let end = arr.get(1)?.as_str()?.to_string();
            Some((start, end))
        });

    let apply_group_by = v
        .get("apply_group_by")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    Ok(SemanticQuerySpec {
        metrics,
        group_by,
        where_filters,
        order_by,
        limit,
        time_constraint,
        apply_group_by,
    })
}

pub fn parse_group_by_str(s: &str) -> Option<GroupBySpec> {
    // TimeDimension('metric_time', 'day')
    if let Some(inner) = s
        .strip_prefix("TimeDimension(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let parts: Vec<&str> = inner.split(',').collect();
        let name = parts.first()?.trim().trim_matches('\'').trim_matches('"');
        let granularity = parts
            .get(1)
            .map(|g| g.trim().trim_matches('\'').trim_matches('"'))
            .unwrap_or("day");
        if !is_valid_identifier(name) || !is_valid_identifier(granularity) {
            return None;
        }
        // Optional date_part: TimeDimension('metric_time', 'day', date_part='year')
        let date_part = parts.iter().skip(2).find_map(|p| {
            let trimmed = p.trim();
            trimmed
                .strip_prefix("date_part=")
                .or_else(|| trimmed.strip_prefix("date_part ="))
                .map(|v| v.trim().trim_matches('\'').trim_matches('"').to_string())
        });
        return Some(GroupBySpec::TimeDimension {
            name: name.to_string(),
            granularity: granularity.to_string(),
            date_part,
        });
    }

    // Dimension('entity__name') or Dimension('entity1__entity2__name') or Dimension('name')
    if let Some(inner) = s
        .strip_prefix("Dimension(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let dim_ref = inner.trim().trim_matches('\'').trim_matches('"');
        // Use rsplit_once to split off the last segment as the dimension name.
        // For multi-hop: account_id__customer_id__customer_name →
        //   entity = "account_id__customer_id", name = "customer_name"
        if let Some((entity, name)) = dim_ref.rsplit_once("__") {
            if entity.split("__").all(is_valid_identifier) && is_valid_identifier(name) {
                return Some(GroupBySpec::Dimension {
                    entity: Some(entity.to_string()),
                    name: name.to_string(),
                });
            }
        }
        if !is_valid_identifier(dim_ref) {
            return None;
        }
        return Some(GroupBySpec::Dimension {
            entity: None,
            name: dim_ref.to_string(),
        });
    }

    // Entity('listing')
    if let Some(inner) = s.strip_prefix("Entity(").and_then(|s| s.strip_suffix(')')) {
        let entity_name = inner.trim().trim_matches('\'').trim_matches('"');
        if !is_valid_identifier(entity_name) {
            return None;
        }
        return Some(GroupBySpec::Entity {
            name: entity_name.to_string(),
        });
    }

    // Colon shorthand for time dimensions: metric_time:day, metric_time:week
    // Also supports entity prefix: order_id__metric_time:month
    if let Some((prefix, granularity)) = s.split_once(':') {
        if !is_valid_identifier(granularity) {
            return None;
        }
        let name = if let Some((_entity, dim_name)) = prefix.split_once("__") {
            if !is_valid_identifier(dim_name) {
                return None;
            }
            dim_name
        } else {
            if !is_valid_identifier(prefix) {
                return None;
            }
            prefix
        };
        return Some(GroupBySpec::TimeDimension {
            name: name.to_string(),
            granularity: granularity.to_string(),
            date_part: None,
        });
    }

    // Double-underscore with granularity: metric_time__day → TimeDimension
    // This is the standard MetricFlow JSON notation for time dimensions.
    if let Some((prefix, suffix)) = s.split_once("__") {
        if VALID_GRANULARITIES.contains(&suffix) {
            if !is_valid_identifier(prefix) {
                return None;
            }
            return Some(GroupBySpec::TimeDimension {
                name: prefix.to_string(),
                granularity: suffix.to_string(),
                date_part: None,
            });
        }
    }

    // Plain string: treat as dimension name
    if let Some((entity, name)) = s.split_once("__") {
        if !is_valid_identifier(entity) || !is_valid_identifier(name) {
            return None;
        }
        Some(GroupBySpec::Dimension {
            entity: Some(entity.to_string()),
            name: name.to_string(),
        })
    } else {
        if !is_valid_identifier(s) {
            return None;
        }
        Some(GroupBySpec::Dimension {
            entity: None,
            name: s.to_string(),
        })
    }
}

pub fn parse_order_by_str(s: &str) -> Option<OrderBySpec> {
    let (name, descending) = if let Some(name) = s.strip_prefix('-') {
        (name, true)
    } else if let Some(name) = s.strip_prefix('+') {
        (name, false)
    } else {
        (s, false)
    };
    // Validate identifier: only alphanumeric + underscore allowed.
    if !is_valid_identifier(name) {
        return None;
    }
    Some(OrderBySpec {
        name: name.to_string(),
        descending,
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// Metric resolution — read from MetricStore
// ═══════════════════════════════════════════════════════════════════════════

fn resolve_metric(
    store: &mut impl MetricStore,
    name: &str,
) -> Result<ResolvedMetric, MetricFlowError> {
    let row = store.lookup_metric(name)?.ok_or_else(|| {
        let avail = store.list_metric_names().unwrap_or_default();
        let hint = if avail.is_empty() {
            String::new()
        } else {
            format!("\nAvailable metrics: {}", avail.join(", "))
        };
        MetricFlowError::Other(format!("metric not found: {name}{hint}"))
    })?;

    let metric_type_str = row.metric_type;
    let description = row.description;
    let type_params_json = row.type_params;
    let metric_filter_json = row.metric_filter;
    let metric_time_granularity = row.time_granularity;

    let metric_type = match metric_type_str.as_str() {
        "simple" => MetricType::Simple,
        "derived" => MetricType::Derived,
        "ratio" => MetricType::Ratio,
        "cumulative" => MetricType::Cumulative,
        "conversion" => MetricType::Conversion,
        other => {
            return Err(MetricFlowError::Other(format!(
                "unknown metric type for {name}: {other}"
            )));
        }
    };

    let tp: serde_json::Value = if type_params_json.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&type_params_json).unwrap_or(serde_json::Value::Null)
    };

    // Parse metric-level filters.
    let metric_filters = parse_metric_filters(&metric_filter_json);

    // Parse aggregation params for simple metrics.
    let agg_params = tp.get("metric_aggregation_params").and_then(|p| {
        if p.is_null() {
            return None;
        }
        Some(AggParams {
            semantic_model: p
                .get("semantic_model")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            agg: p
                .get("agg")
                .and_then(|v| v.as_str())
                .unwrap_or("sum")
                .to_string(),
            expr: p
                .get("expr")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                // Fallback: some manifest formats store the measure expr at
                // type_params.expr rather than metric_aggregation_params.expr.
                .or_else(|| {
                    tp.get("expr")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                })
                .unwrap_or("")
                .to_string(),
            agg_time_dimension: p
                .get("agg_time_dimension")
                .and_then(|v| v.as_str())
                .map(String::from),
            non_additive_dimension: p
                .get("non_additive_dimension")
                .filter(|v| !v.is_null())
                .cloned(),
            percentile: p
                .get("agg_params")
                .and_then(|ap| ap.get("percentile"))
                .and_then(|v| v.as_f64()),
            use_discrete_percentile: p
                .get("agg_params")
                .and_then(|ap| ap.get("use_discrete_percentile"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        })
    });

    // Parse derived metric expression and input metrics.
    let derived_expr = tp.get("expr").and_then(|v| v.as_str()).map(String::from);

    let input_metrics = tp
        .get("metrics")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let name = m.get("name")?.as_str()?.to_string();
                    let filters = m
                        .get("filter")
                        .map(parse_input_metric_filters)
                        .unwrap_or_default();
                    let alias = m.get("alias").and_then(|a| a.as_str()).map(String::from);
                    let offset_window = m.get("offset_window").and_then(|o| {
                        if let Some(s) = o.as_str() {
                            Some(s.to_string())
                        } else {
                            let count = o.get("count").and_then(|c| c.as_u64())?;
                            let gran = o.get("granularity").and_then(|g| g.as_str())?;
                            Some(format!("{count} {gran}"))
                        }
                    });
                    let offset_to_grain = m
                        .get("offset_to_grain")
                        .and_then(|o| o.as_str())
                        .map(String::from);
                    Some(MetricInput {
                        name,
                        filters,
                        alias,
                        offset_window,
                        offset_to_grain,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    // New-style cumulative metrics store their input metric inside
    // cumulative_type_params.metric rather than in the top-level metrics array.
    // Merge it into input_metrics so downstream lookup can find agg_params.
    let mut input_metrics: Vec<MetricInput> = input_metrics;
    if let Some(m) = tp
        .get("cumulative_type_params")
        .and_then(|c| c.get("metric"))
        .filter(|m| !m.is_null())
    {
        if let Some(name) = m.get("name").and_then(|n| n.as_str()) {
            if !input_metrics.iter().any(|im| im.name == name) {
                input_metrics.push(MetricInput {
                    name: name.to_string(),
                    filters: m
                        .get("filter")
                        .map(parse_input_metric_filters)
                        .unwrap_or_default(),
                    alias: m.get("alias").and_then(|a| a.as_str()).map(String::from),
                    offset_window: None,
                    offset_to_grain: None,
                });
            }
        }
    }

    // Parse ratio metric numerator/denominator.
    let numerator = tp.get("numerator").and_then(|n| {
        if n.is_null() {
            return None;
        }
        Some(MetricInput {
            name: n.get("name")?.as_str()?.to_string(),
            filters: n
                .get("filter")
                .map(parse_input_metric_filters)
                .unwrap_or_default(),
            alias: n.get("alias").and_then(|a| a.as_str()).map(String::from),
            offset_window: None,
            offset_to_grain: None,
        })
    });

    let denominator = tp.get("denominator").and_then(|d| {
        if d.is_null() {
            return None;
        }
        Some(MetricInput {
            name: d.get("name")?.as_str()?.to_string(),
            filters: d
                .get("filter")
                .map(parse_input_metric_filters)
                .unwrap_or_default(),
            alias: d.get("alias").and_then(|a| a.as_str()).map(String::from),
            offset_window: None,
            offset_to_grain: None,
        })
    });

    // Cumulative params.
    let cumulative_params = tp.get("cumulative_type_params").and_then(|c| {
        if c.is_null() {
            return None;
        }
        let window = c.get("window");
        Some(CumulativeParams {
            window_count: window.and_then(|w| w.get("count")).and_then(|c| c.as_i64()),
            window_granularity: window
                .and_then(|w| w.get("granularity"))
                .and_then(|g| g.as_str())
                .map(String::from),
            grain_to_date: c
                .get("grain_to_date")
                .and_then(|g| g.as_str())
                .map(String::from),
        })
    });

    // Conversion params.
    let conversion_params = tp.get("conversion_type_params").and_then(|c| {
        if c.is_null() {
            return None;
        }
        let window = c.get("window");
        let const_props = c
            .get("constant_properties")
            .and_then(|cp| cp.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|p| {
                        let base = p.get("base_property")?.as_str()?.to_string();
                        let conv = p.get("conversion_property")?.as_str()?.to_string();
                        Some((base, conv))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(ConversionParams {
            entity: c
                .get("entity")
                .and_then(|e| e.as_str())
                .unwrap_or("")
                .to_string(),
            base_metric: c
                .get("base_metric")
                .and_then(|b| b.get("name"))
                .and_then(|n| n.as_str())
                .or_else(|| {
                    c.get("base_measure")
                        .and_then(|b| b.get("name"))
                        .and_then(|n| n.as_str())
                })
                .unwrap_or("")
                .to_string(),
            conversion_metric: c
                .get("conversion_metric")
                .and_then(|b| b.get("name"))
                .and_then(|n| n.as_str())
                .or_else(|| {
                    c.get("conversion_measure")
                        .and_then(|b| b.get("name"))
                        .and_then(|n| n.as_str())
                })
                .unwrap_or("")
                .to_string(),
            calculation: c
                .get("calculation")
                .and_then(|v| v.as_str())
                .unwrap_or("conversion_rate")
                .to_string(),
            window_count: window.and_then(|w| w.get("count")).and_then(|c| c.as_i64()),
            window_granularity: window
                .and_then(|w| w.get("granularity"))
                .and_then(|g| g.as_str())
                .map(String::from),
            constant_properties: const_props,
        })
    });

    let join_to_timespine = tp
        .get("join_to_timespine")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let fill_nulls_with = tp.get("fill_nulls_with").and_then(|v| v.as_i64());

    Ok(ResolvedMetric {
        name: name.to_string(),
        metric_type,
        description,
        agg_params,
        metric_filters,
        derived_expr,
        input_metrics,
        numerator,
        denominator,
        cumulative_params,
        conversion_params,
        join_to_timespine,
        fill_nulls_with,
        time_granularity: metric_time_granularity,
    })
}

fn parse_metric_filters(filter_json: &str) -> Vec<String> {
    if filter_json.is_empty() {
        return vec![];
    }
    let v: serde_json::Value = match serde_json::from_str(filter_json) {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    if v.is_null() {
        return vec![];
    }
    // WhereFilterIntersection: { "where_filters": [{"where_sql_template": "..."}] }
    v.get("where_filters")
        .and_then(|wf| wf.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|f| {
                    f.get("where_sql_template")
                        .and_then(|t| t.as_str())
                        .map(|s| s.trim().to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_input_metric_filters(v: &serde_json::Value) -> Vec<String> {
    if v.is_null() {
        return vec![];
    }
    v.get("where_filters")
        .and_then(|wf| wf.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|f| {
                    f.get("where_sql_template")
                        .and_then(|t| t.as_str())
                        .map(|s| s.trim().to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Resolve a semantic model by name from the store.
fn resolve_model(
    store: &mut impl MetricStore,
    name: &str,
) -> Result<ResolvedModel, MetricFlowError> {
    let row = store
        .lookup_semantic_model(name)?
        .ok_or_else(|| MetricFlowError::Other(format!("semantic model not found: {name}")))?;

    let node_relation_json = row.node_relation;
    let primary_entity = row.primary_entity;
    let unique_id = row.unique_id;
    let scd_valid_from = row.scd_valid_from;
    let scd_valid_to = row.scd_valid_to;

    let nr: serde_json::Value =
        serde_json::from_str(&node_relation_json).unwrap_or(serde_json::Value::Null);

    let relation_name = nr
        .get("relation_name")
        .and_then(|v| v.as_str())
        .unwrap_or(name)
        .to_string();
    let alias = nr
        .get("alias")
        .and_then(|v| v.as_str())
        .unwrap_or(name)
        .to_string();
    let schema_name = nr
        .get("schema_name")
        .and_then(|v| v.as_str())
        .unwrap_or("public")
        .to_string();
    let database = nr
        .get("database")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let primary_entity = if primary_entity.is_empty() {
        None
    } else {
        Some(primary_entity)
    };

    // Get entities.
    let ent_rows = store.lookup_model_entities(&unique_id)?;
    let entities: Vec<EntityDef> = ent_rows
        .into_iter()
        .map(|r| {
            let expr = if r.expr.is_empty() {
                r.name.clone()
            } else {
                r.expr
            };
            EntityDef {
                name: r.name,
                entity_type: r.entity_type,
                expr,
            }
        })
        .collect();

    // Get dimensions.
    let dim_rows = store.lookup_model_dimensions(&unique_id)?;
    let dimensions: Vec<DimensionDef> = dim_rows
        .into_iter()
        .map(|r| {
            let expr = if r.expr.is_empty() {
                r.name.clone()
            } else {
                r.expr
            };
            DimensionDef {
                name: r.name,
                dimension_type: r.dimension_type,
                expr,
                time_granularity: if r.time_granularity.is_empty() {
                    None
                } else {
                    Some(r.time_granularity)
                },
                is_partition: r.is_partition,
            }
        })
        .collect();

    Ok(ResolvedModel {
        name: name.to_string(),
        relation_name,
        alias,
        schema_name,
        database,
        primary_entity,
        entities,
        dimensions,
        scd_valid_from,
        scd_valid_to,
    })
}

/// Find the semantic model that owns a given entity as primary/unique.
fn find_model_for_entity_pk(
    store: &mut impl MetricStore,
    entity_name: &str,
) -> Result<Option<String>, MetricFlowError> {
    store.find_model_for_entity(entity_name, true)
}

/// Like `find_model_for_entity_pk` but matches any entity type (including foreign).
fn find_model_for_entity_any(
    store: &mut impl MetricStore,
    entity_name: &str,
) -> Result<Option<String>, MetricFlowError> {
    store.find_model_for_entity(entity_name, false)
}

/// Find a model that has `entity_name` as primary/unique AND contains `dim_name`
/// (as either a dimension or an entity). Falls back to any primary/unique model.
fn find_model_for_entity_and_dim(
    store: &mut impl MetricStore,
    entity_name: &str,
    dim_name: &str,
) -> Result<Option<String>, MetricFlowError> {
    let join_rows = store.lookup_all_join_graph_entities()?;
    let candidates: Vec<&str> = join_rows
        .iter()
        .filter(|r| {
            r.entity_name == entity_name
                && matches!(r.entity_type.as_str(), "primary" | "unique" | "natural")
        })
        .map(|r| r.model_name.as_str())
        .collect();
    for model_name in &candidates {
        if let Some(model_row) = store.lookup_semantic_model(model_name)? {
            let dims = store.lookup_model_dimensions(&model_row.unique_id)?;
            if dims.iter().any(|d| d.name == dim_name) {
                return Ok(Some((*model_name).to_string()));
            }
            let ents = store.lookup_model_entities(&model_row.unique_id)?;
            if ents.iter().any(|e| e.name == dim_name) {
                return Ok(Some((*model_name).to_string()));
            }
        }
    }
    if let Some(first) = candidates.first() {
        return Ok(Some((*first).to_string()));
    }
    Ok(None)
}

/// Find the best "dimension/mapping" model for an entity — prefers models with
/// no dimensions (pure bridge/mapping tables) over models with measures.
/// `exclude` lists model names to skip (e.g., models backing metric subqueries).
fn find_dimension_model_for_entity(
    store: &mut impl MetricStore,
    entity_name: &str,
    exclude: &[&str],
) -> Result<Option<String>, MetricFlowError> {
    let join_rows = store.lookup_all_join_graph_entities()?;
    let candidates: Vec<&str> = join_rows
        .iter()
        .filter(|r| {
            r.entity_name == entity_name
                && matches!(r.entity_type.as_str(), "primary" | "unique" | "natural")
                && !exclude.contains(&r.model_name.as_str())
        })
        .map(|r| r.model_name.as_str())
        .collect();
    let mut best: Option<(String, usize)> = None;
    for model_name in &candidates {
        if let Some(model_row) = store.lookup_semantic_model(model_name)? {
            let dims = store.lookup_model_dimensions(&model_row.unique_id)?;
            let score = dims.len();
            if best.as_ref().is_none_or(|(_, s)| score < *s) {
                best = Some(((*model_name).to_string(), score));
            }
        }
    }
    Ok(best.map(|(name, _)| name))
}

/// Load time spine from the store, if any.
fn load_time_spine(store: &mut impl MetricStore) -> Option<TimeSpine> {
    let row = store.lookup_time_spine().ok()??;

    let nr: serde_json::Value = serde_json::from_str(&row.node_relation).ok()?;
    let relation_name = nr
        .get("relation_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            // Construct from schema + alias when relation_name is null.
            let alias = nr.get("alias")?.as_str()?;
            let schema = nr.get("schema_name").and_then(|v| v.as_str());
            Some(if let Some(s) = schema {
                format!("\"{s}\".\"{alias}\"")
            } else {
                format!("\"{alias}\"")
            })
        })?;

    Some(TimeSpine {
        relation_name,
        primary_column: row.primary_column,
        primary_granularity: row.primary_granularity,
        custom_granularities: row.custom_granularities,
    })
}

fn parse_time_spine_row(row: &RawTimeSpineRow) -> Option<TimeSpine> {
    let nr: serde_json::Value = serde_json::from_str(&row.node_relation).ok()?;
    let relation_name = nr
        .get("relation_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            let alias = nr.get("alias")?.as_str()?;
            let schema = nr.get("schema_name").and_then(|v| v.as_str());
            Some(if let Some(s) = schema {
                format!("\"{s}\".\"{alias}\"")
            } else {
                format!("\"{alias}\"")
            })
        })?;
    Some(TimeSpine {
        relation_name,
        primary_column: row.primary_column.clone(),
        primary_granularity: row.primary_granularity.clone(),
        custom_granularities: row.custom_granularities.clone(),
    })
}

fn granularity_rank(gran: &str) -> u8 {
    match gran {
        "nanosecond" => 1,
        "microsecond" => 2,
        "millisecond" => 3,
        "second" => 4,
        "minute" => 5,
        "hour" => 6,
        "day" => 7,
        "week" => 8,
        "month" => 9,
        "quarter" => 10,
        "year" => 11,
        _ => 7,
    }
}

fn load_all_time_spines_from_store(store: &mut impl MetricStore) -> Vec<TimeSpine> {
    store
        .lookup_all_time_spines()
        .unwrap_or_default()
        .iter()
        .filter_map(parse_time_spine_row)
        .collect()
}

fn is_standard_granularity(gran: &str) -> bool {
    matches!(
        gran,
        "nanosecond"
            | "microsecond"
            | "millisecond"
            | "second"
            | "minute"
            | "hour"
            | "day"
            | "week"
            | "month"
            | "quarter"
            | "year"
    )
}

fn find_custom_granularity_spine<'a>(
    spines: &'a [TimeSpine],
    gran_name: &str,
) -> Option<(&'a TimeSpine, &'a str)> {
    for spine in spines {
        for (name, column) in &spine.custom_granularities {
            if name == gran_name {
                return Some((spine, column.as_str()));
            }
        }
    }
    None
}

fn pick_time_spine_for_granularity<'a>(
    spines: &'a [TimeSpine],
    query_gran: &str,
) -> Option<&'a TimeSpine> {
    let query_rank = granularity_rank(query_gran);
    // Pick the spine whose primary_granularity is <= query_gran and closest to it.
    spines
        .iter()
        .filter(|s| granularity_rank(&s.primary_granularity) <= query_rank)
        .max_by_key(|s| granularity_rank(&s.primary_granularity))
}

// ═══════════════════════════════════════════════════════════════════════════
// Entity join graph
// ═══════════════════════════════════════════════════════════════════════════

/// Build the join graph from entity relationships across all semantic models.
fn build_join_graph(store: &mut impl MetricStore) -> Result<Vec<JoinEdge>, MetricFlowError> {
    let rows = store.lookup_all_join_graph_entities()?;

    struct EntityInfo {
        model_name: String,
        entity_type: String,
        expr: String,
    }

    let mut entity_map: HashMap<String, Vec<EntityInfo>> = HashMap::new();
    for r in &rows {
        entity_map
            .entry(r.entity_name.clone())
            .or_default()
            .push(EntityInfo {
                model_name: r.model_name.clone(),
                entity_type: r.entity_type.clone(),
                expr: if r.expr.is_empty() {
                    r.entity_name.clone()
                } else {
                    r.expr.clone()
                },
            });
    }

    let mut edges = Vec::new();

    for (entity_name, infos) in &entity_map {
        // Find primary/unique side (PK) and foreign side (FK).
        let pk_models: Vec<&EntityInfo> = infos
            .iter()
            .filter(|i| {
                i.entity_type == "primary"
                    || i.entity_type == "unique"
                    || i.entity_type == "natural"
            })
            .collect();
        let fk_models: Vec<&EntityInfo> = infos
            .iter()
            .filter(|i| i.entity_type == "foreign")
            .collect();

        // Create bidirectional edges between FK and PK models so
        // find_join_path works regardless of which model is primary.
        for fk in &fk_models {
            for pk in &pk_models {
                if fk.model_name != pk.model_name {
                    edges.push(JoinEdge {
                        from_model: fk.model_name.clone(),
                        to_model: pk.model_name.clone(),
                        from_expr: fk.expr.clone(),
                        to_expr: pk.expr.clone(),
                        entity_name: entity_name.clone(),
                    });
                    edges.push(JoinEdge {
                        from_model: pk.model_name.clone(),
                        to_model: fk.model_name.clone(),
                        from_expr: pk.expr.clone(),
                        to_expr: fk.expr.clone(),
                        entity_name: entity_name.clone(),
                    });
                }
            }
        }

        // Also create edges between primary entities of different models
        // (for models that share a primary entity at the same grain).
        if pk_models.len() > 1 {
            for i in 0..pk_models.len() {
                for j in (i + 1)..pk_models.len() {
                    edges.push(JoinEdge {
                        from_model: pk_models[i].model_name.clone(),
                        to_model: pk_models[j].model_name.clone(),
                        from_expr: pk_models[i].expr.clone(),
                        to_expr: pk_models[j].expr.clone(),
                        entity_name: entity_name.clone(),
                    });
                    // Bidirectional.
                    edges.push(JoinEdge {
                        from_model: pk_models[j].model_name.clone(),
                        to_model: pk_models[i].model_name.clone(),
                        from_expr: pk_models[j].expr.clone(),
                        to_expr: pk_models[i].expr.clone(),
                        entity_name: entity_name.clone(),
                    });
                }
            }
        }
    }

    Ok(edges)
}

/// Find the shortest join path from one model to another using BFS.
fn find_join_path(edges: &[JoinEdge], from: &str, to: &str) -> Option<Vec<JoinEdge>> {
    if from == to {
        return Some(vec![]);
    }

    // Build adjacency list.
    let mut adj: HashMap<&str, Vec<&JoinEdge>> = HashMap::new();
    for edge in edges {
        adj.entry(edge.from_model.as_str()).or_default().push(edge);
    }

    // BFS.
    let mut visited: HashSet<&str> = HashSet::new();
    visited.insert(from);
    let mut queue: Vec<(String, Vec<JoinEdge>)> = vec![(from.to_string(), vec![])];

    while let Some((current, path)) = queue.first().cloned() {
        queue.remove(0);

        if let Some(neighbors) = adj.get(current.as_str()) {
            for edge in neighbors {
                if edge.to_model == to {
                    let mut full_path = path;
                    full_path.push((*edge).clone());
                    return Some(full_path);
                }
                if !visited.contains(edge.to_model.as_str()) {
                    visited.insert(
                        // Extend lifetime via the edge slice.
                        edges
                            .iter()
                            .find(|e| e.to_model == edge.to_model)
                            .map(|e| e.to_model.as_str())
                            .unwrap_or(""),
                    );
                    let mut new_path = path.clone();
                    new_path.push((*edge).clone());
                    queue.push((edge.to_model.clone(), new_path));
                }
            }
        }
    }

    None
}

/// Build multi-hop subquery joins for entity chains with >1 segment.
///
/// Returns a list of `MultiHopSubquery` structs, each representing a
/// subquery that chains intermediate tables to a leaf model. The subquery
/// is joined to the fact table independently, producing correct Cartesian
/// product semantics when multiple dimensions come from different leaf
/// models along the same entity chain.
#[allow(clippy::cognitive_complexity)]
fn plan_multi_hop_joins(
    spec: &SemanticQuerySpec,
    primary_model_name: &str,
    model_aliases: &HashMap<String, (String, &ResolvedModel)>,
    join_edges: &[JoinEdge],
    dialect: Dialect,
    metric_filters: &[String],
) -> Vec<MultiHopSubquery> {
    let mut result = Vec::new();
    let mut counter = 0usize;

    // Collect multi-hop (entity_chain, dim_name) pairs from group-by and filters.
    let mut multi_hop_dims: Vec<(String, String)> = Vec::new();
    // Also collect single-hop (entity, dim_name) pairs — these may be absorbed
    // into a multi-hop subquery when the entity is an intermediate model.
    let mut single_hop_dims: Vec<(String, String)> = Vec::new();

    for gb in &spec.group_by {
        if let GroupBySpec::Dimension {
            entity: Some(entity_name),
            name,
        } = gb
        {
            if entity_name.contains("__") {
                multi_hop_dims.push((entity_name.clone(), name.clone()));
            } else {
                single_hop_dims.push((entity_name.clone(), name.clone()));
            }
        }
    }

    // Also collect from filters.
    for filter in metric_filters.iter().chain(spec.where_filters.iter()) {
        let mut cursor = 0usize;
        while let Some(pos) = filter[cursor..].find("Dimension(") {
            let abs = cursor + pos;
            let preceded_by_alpha = abs > 0 && filter.as_bytes()[abs - 1].is_ascii_alphabetic();
            if preceded_by_alpha {
                cursor = abs + 10;
                continue;
            }
            let inner_start = abs + 10;
            if let Some(paren_end) = filter[inner_start..].find(')') {
                let dim_ref = filter[inner_start..inner_start + paren_end]
                    .trim()
                    .trim_matches('\'')
                    .trim_matches('"');
                if let Some((chain, dim_name)) = dim_ref.rsplit_once("__") {
                    if chain.contains("__") {
                        let entity_chain = chain.to_string();
                        if !multi_hop_dims
                            .iter()
                            .any(|(e, d)| *e == entity_chain && *d == dim_name)
                        {
                            multi_hop_dims.push((entity_chain, dim_name.to_string()));
                        }
                    }
                }
                cursor = inner_start + paren_end + 1;
            } else {
                break;
            }
        }
    }

    if multi_hop_dims.is_empty() {
        return result;
    }

    // Group by (entity_chain, leaf_model) — dimensions from the same leaf model
    // can share a subquery.
    let mut groups: Vec<(String, String, Vec<String>)> = Vec::new(); // (entity_chain, leaf_model, dim_names)

    for (entity_chain, dim_name) in &multi_hop_dims {
        let target_entity = entity_chain
            .rsplit_once("__")
            .map_or(entity_chain.as_str(), |(_, last)| last);

        // Find the leaf model that has this dimension AND the target entity.
        let mut leaf_model_name = None;
        for (model_name, (_alias, model)) in model_aliases {
            if model_name == primary_model_name {
                continue;
            }
            let has_entity = model.entities.iter().any(|e| e.name == target_entity)
                || model.primary_entity.as_deref() == Some(target_entity);
            if !has_entity {
                continue;
            }
            let has_dim = model.dimensions.iter().any(|d| d.name == *dim_name)
                || model.entities.iter().any(|e| e.name == *dim_name);
            if has_dim {
                leaf_model_name = Some(model_name.clone());
                break;
            }
        }

        if let Some(ref leaf) = leaf_model_name {
            if let Some(group) = groups
                .iter_mut()
                .find(|(ec, lm, _)| ec == entity_chain && lm == leaf)
            {
                if !group.2.contains(dim_name) {
                    group.2.push(dim_name.clone());
                }
            } else {
                groups.push((entity_chain.clone(), leaf.clone(), vec![dim_name.clone()]));
            }
        }
    }

    // Check whether multiple groups share an entity chain — that's the case
    // that requires independent subqueries.
    // Single-group chains can use flat joins.
    let chain_counts: HashMap<&str, usize> = {
        let mut m = HashMap::new();
        for (chain, _, _) in &groups {
            *m.entry(chain.as_str()).or_insert(0) += 1;
        }
        m
    };

    let needs_subqueries = chain_counts.values().any(|&c| c > 1);
    if !needs_subqueries {
        return result;
    }

    for (_entity_chain, leaf_model_name, dim_names) in &groups {
        // Find the join path from primary to leaf.
        let path = match find_join_path(join_edges, primary_model_name, leaf_model_name) {
            Some(p) if p.len() >= 2 => p,
            _ => continue,
        };

        // The first edge connects primary to the intermediate model.
        let first_edge = &path[0];

        // Get the primary model's alias for the fact join expression.
        let primary_alias = model_aliases
            .get(primary_model_name)
            .map(|(a, _)| a.as_str())
            .unwrap_or("t");

        let fact_join_expr = format!("{primary_alias}.{}", first_edge.from_expr);

        // Build the subquery: SELECT * FROM intermediate LEFT JOIN ... LEFT JOIN leaf
        let mut subquery_parts = Vec::new();
        let intermediate_model = &path[0].to_model;
        let intermediate_relation = model_aliases
            .get(intermediate_model)
            .map(|(_, m)| render_full_relation(m, dialect))
            .unwrap_or_else(|| format!("\"{intermediate_model}\""));
        let intermediate_alias = model_aliases
            .get(intermediate_model)
            .map(|(a, _)| a.clone())
            .unwrap_or_else(|| format!("__mhi{counter}"));

        subquery_parts.push(format!(
            "SELECT * FROM {intermediate_relation} AS {intermediate_alias}"
        ));

        for edge in &path[1..] {
            let to_relation = model_aliases
                .get(&edge.to_model)
                .map(|(_, m)| render_full_relation(m, dialect))
                .unwrap_or_else(|| format!("\"{}\"", edge.to_model));
            let to_alias = model_aliases
                .get(&edge.to_model)
                .map(|(a, _)| a.clone())
                .unwrap_or_else(|| edge.to_model.clone());
            let from_alias = model_aliases
                .get(&edge.from_model)
                .map(|(a, _)| a.clone())
                .unwrap_or_else(|| edge.from_model.clone());
            subquery_parts.push(format!(
                " LEFT JOIN {to_relation} AS {to_alias} ON {from_alias}.{} = {to_alias}.{}",
                edge.from_expr, edge.to_expr
            ));
        }

        let subquery_sql = subquery_parts.join("");
        let alias = format!("__mh{counter}");
        counter += 1;

        // The subquery join column is the first edge's to_expr (the intermediate
        // model's column that connects to the fact table).
        let subquery_join_col = first_edge.to_expr.clone();

        // Build dim_columns map: dimension name → column in the subquery.
        let mut dim_columns = HashMap::new();
        let leaf_model = model_aliases.get(leaf_model_name.as_str()).map(|(_, m)| *m);
        if let Some(model) = leaf_model {
            for dn in dim_names {
                if let Some(dim) = model.dimensions.iter().find(|d| &d.name == dn) {
                    dim_columns.insert(dn.clone(), format!("{alias}.{}", dim.expr));
                } else if let Some(ent) = model.entities.iter().find(|e| &e.name == dn) {
                    dim_columns.insert(dn.clone(), format!("{alias}.{}", ent.expr));
                }
            }
        }

        // Also add columns from intermediate models (for mixed-length joins
        // like `account_id__extra_dim` where `extra_dim` is in bridge_table).
        for edge in &path {
            if let Some((_, model)) = model_aliases.get(&edge.to_model) {
                for dn in dim_names {
                    if !dim_columns.contains_key(dn.as_str()) {
                        if let Some(dim) = model.dimensions.iter().find(|d| &d.name == dn) {
                            dim_columns.insert(dn.clone(), format!("{alias}.{}", dim.expr));
                        }
                    }
                }
            }
        }

        result.push(MultiHopSubquery {
            alias,
            subquery_sql,
            fact_join_expr,
            subquery_join_col,
            dim_columns,
        });
    }

    // Absorb single-hop dimensions into existing subqueries when the entity
    // references an intermediate model in one of the multi-hop paths.
    // This avoids double-counting from joining bridge_table both flat and as
    // part of a subquery.
    for (entity_name, dim_name) in &single_hop_dims {
        // Check if any subquery has a path through a model containing this entity+dim.
        // If so, add the dimension to that subquery's dim_columns.
        let already_handled = result
            .iter()
            .any(|mh| mh.dim_columns.contains_key(dim_name.as_str()));
        if already_handled {
            continue;
        }
        // Find a subquery whose path includes a model with this entity and dimension.
        for mh in result.iter_mut() {
            // The subquery's SQL includes intermediate models. Check model_aliases
            // for models that have this entity and dimension.
            for (model_name, (_alias, model)) in model_aliases {
                if model_name == primary_model_name {
                    continue;
                }
                let has_entity = model.entities.iter().any(|e| e.name == *entity_name)
                    || model.primary_entity.as_deref() == Some(entity_name.as_str());
                if !has_entity {
                    continue;
                }
                let dim_entry = model.dimensions.iter().find(|d| d.name == *dim_name);
                if dim_entry.is_none() {
                    continue;
                }
                // Check if this model is part of this subquery's path
                // (the subquery_sql contains the model's relation).
                let model_relation = render_full_relation(model, dialect);
                if mh.subquery_sql.contains(&model_relation) {
                    if let Some(dim) = dim_entry {
                        mh.dim_columns
                            .insert(dim_name.clone(), format!("{}.{}", mh.alias, dim.expr));
                        break;
                    }
                }
            }
            if mh.dim_columns.contains_key(dim_name.as_str()) {
                break;
            }
        }
    }

    result
}

// ═══════════════════════════════════════════════════════════════════════════
// Where-filter template resolution
// ═══════════════════════════════════════════════════════════════════════════

#[allow(clippy::too_many_arguments)]
fn resolve_where_filter_custom_gran(
    template: &str,
    model_aliases: &HashMap<String, (String, &ResolvedModel)>,
    dialect: Dialect,
    primary_model_name: &str,
    all_time_spines: &[TimeSpine],
    agg_time_dim: Option<&str>,
    custom_gran_joins: &mut Vec<(String, String, String)>,
    cg_alias_counter: &mut u32,
    _cte_sql: &mut String,
) -> String {
    let mut result = template.to_string();
    while let Some(start) = result.find("{{ TimeDimension(") {
        let Some(end) = result[start..].find("}}").map(|i| start + i + 2) else {
            break;
        };
        let inner = &result[start + 2..end - 2].trim();
        let td_ref = inner
            .strip_prefix("TimeDimension(")
            .and_then(|s| s.strip_suffix(')'))
            .unwrap_or(inner);
        let parts: Vec<&str> = td_ref.split(',').collect();
        let name = parts
            .first()
            .unwrap_or(&"metric_time")
            .trim()
            .trim_matches('\'')
            .trim_matches('"');
        let granularity = parts
            .get(1)
            .unwrap_or(&"day")
            .trim()
            .trim_matches('\'')
            .trim_matches('"');

        if !is_standard_granularity(granularity) {
            if let Some((spine, custom_col)) =
                find_custom_granularity_spine(all_time_spines, granularity)
            {
                let existing = custom_gran_joins
                    .iter()
                    .find(|(_, _, on)| on.contains(&format!(".{custom_col}")));
                let alias = if let Some((a, _, _)) = existing {
                    a.clone()
                } else {
                    let cg_alias = if *cg_alias_counter == 0 {
                        "ts_cg".to_string()
                    } else {
                        format!("ts_cg{cg_alias_counter}")
                    };
                    *cg_alias_counter += 1;
                    let time_expr = resolve_time_dimension_ref_with_agg(
                        name,
                        "day",
                        model_aliases,
                        dialect,
                        primary_model_name,
                        agg_time_dim,
                    );
                    let spine_rel = match dialect {
                        Dialect::Databricks => spine.relation_name.replace('"', "`"),
                        _ => spine.relation_name.clone(),
                    };
                    let on_cond = format!("{time_expr} = {cg_alias}.{}", spine.primary_column);
                    custom_gran_joins.push((cg_alias.clone(), spine_rel, on_cond));
                    cg_alias
                };
                let resolved = format!("{alias}.{custom_col}");
                result.replace_range(start..end, &resolved);
                continue;
            }
        }
        let resolved = resolve_time_dimension_ref(
            name,
            granularity,
            model_aliases,
            dialect,
            primary_model_name,
        );
        result.replace_range(start..end, &resolved);
    }
    resolve_where_filter(&result, model_aliases, dialect, primary_model_name)
}

/// Resolve Jinja-style dimension references in where filters.
///
/// Converts `{{ Dimension('entity__name') }}` → `alias.column_expr`
/// Converts `{{ TimeDimension('metric_time', 'day') }}` → `DATE_TRUNC('day', alias.column)`
fn resolve_where_filter(
    template: &str,
    model_aliases: &HashMap<String, (String, &ResolvedModel)>,
    dialect: Dialect,
    primary_model_name: &str,
) -> String {
    let mut result = template.to_string();

    // Resolve {{ Dimension('entity__name') }} patterns.
    while let Some(start) = result.find("{{ Dimension(") {
        let Some(end) = result[start..].find("}}").map(|i| start + i + 2) else {
            break;
        };
        let inner = &result[start + 2..end - 2].trim();
        let dim_ref = inner
            .strip_prefix("Dimension(")
            .and_then(|s| s.strip_suffix(')'))
            .unwrap_or(inner)
            .trim()
            .trim_matches('\'')
            .trim_matches('"');

        let resolved = if dim_ref == "metric_time" || dim_ref.ends_with("__metric_time") {
            resolve_time_dimension_ref(dim_ref, "day", model_aliases, dialect, primary_model_name)
        } else {
            resolve_dimension_ref(dim_ref, model_aliases, dialect, primary_model_name)
        };
        result.replace_range(start..end, &resolved);
    }

    // Resolve {{ TimeDimension('name', 'grain') }} patterns.
    while let Some(start) = result.find("{{ TimeDimension(") {
        let Some(end) = result[start..].find("}}").map(|i| start + i + 2) else {
            break;
        };
        let inner = &result[start + 2..end - 2].trim();
        let td_ref = inner
            .strip_prefix("TimeDimension(")
            .and_then(|s| s.strip_suffix(')'))
            .unwrap_or(inner);
        let parts: Vec<&str> = td_ref.split(',').collect();
        let name = parts
            .first()
            .unwrap_or(&"metric_time")
            .trim()
            .trim_matches('\'')
            .trim_matches('"');
        let granularity = parts
            .get(1)
            .unwrap_or(&"day")
            .trim()
            .trim_matches('\'')
            .trim_matches('"');

        let resolved = resolve_time_dimension_ref(
            name,
            granularity,
            model_aliases,
            dialect,
            primary_model_name,
        );
        result.replace_range(start..end, &resolved);
    }

    // Resolve {{ Entity('name') }} patterns → entity column reference.
    while let Some(start) = result.find("{{ Entity(") {
        let Some(end) = result[start..].find("}}").map(|i| start + i + 2) else {
            break;
        };
        let inner = &result[start + 2..end - 2].trim();
        let entity_ref = inner
            .strip_prefix("Entity(")
            .and_then(|s| s.strip_suffix(')'))
            .unwrap_or(inner)
            .trim()
            .trim_matches('\'')
            .trim_matches('"');

        let resolved = resolve_entity_ref(entity_ref, model_aliases, primary_model_name);
        result.replace_range(start..end, &resolved);
    }

    // Resolve {{ Metric('name', ['entity']) }} — replace with the CTE column reference.
    // The CTE is pre-compiled by compile_metric_filter_ctes() and joined by the caller.
    while let Some(start) = result.find("{{ Metric(") {
        let Some(end) = result[start..].find("}}").map(|i| start + i + 2) else {
            break;
        };
        let inner = &result[start + 2..end - 2].trim();
        let metric_ref = inner
            .strip_prefix("Metric(")
            .and_then(|s| s.strip_suffix(')'))
            .unwrap_or(inner)
            .trim();

        // Parse metric name from 'metric_name', ['entity1']
        let metric_name = metric_ref
            .split(',')
            .next()
            .unwrap_or("")
            .trim()
            .trim_matches('\'')
            .trim_matches('"');

        // Parse entity list to build the qualified column name.
        let entity_prefix: String = metric_ref
            .split('[')
            .nth(1)
            .and_then(|s| s.split(']').next())
            .map(|s| {
                s.split(',')
                    .map(|e| e.trim().trim_matches('\'').trim_matches('"'))
                    .filter(|e| !e.is_empty())
                    .collect::<Vec<_>>()
                    .join("__")
            })
            .unwrap_or_default();

        // The CTE alias is __mf_{metric_name}. The column is {entity_prefix}__{metric_name}.
        let col_name = if entity_prefix.is_empty() {
            metric_name.to_string()
        } else {
            format!("{entity_prefix}__{metric_name}")
        };
        let resolved = format!("__mf_{metric_name}.{col_name}");
        result.replace_range(start..end, &resolved);
    }

    // Resolve bare <name>__<gran> time dimension references (not wrapped in Jinja).
    // Matches metric_time__day, revenue_instance__ds__day, etc.
    for gran in VALID_GRANULARITIES {
        let suffix = format!("__{gran}");
        let mut search_start = 0;
        while let Some(pos) = result[search_start..].find(&suffix) {
            let abs_end = search_start + pos + suffix.len();
            // Check that the suffix is followed by a non-identifier character (or EOF).
            let at_boundary =
                abs_end >= result.len() || !result.as_bytes()[abs_end].is_ascii_alphanumeric();
            if !at_boundary {
                search_start = abs_end;
                continue;
            }
            // Walk backwards to find the start of the identifier.
            let ident_start = result[..search_start + pos]
                .rfind(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .map(|i| i + 1)
                .unwrap_or(0);
            let bare = &result[ident_start..abs_end];
            let name = &result[ident_start..search_start + pos];
            if name.is_empty() || name.starts_with('_') {
                search_start = abs_end;
                continue;
            }
            let resolved =
                resolve_time_dimension_ref(name, gran, model_aliases, dialect, primary_model_name);
            if resolved != bare {
                result.replace_range(ident_start..abs_end, &resolved);
                search_start = ident_start + resolved.len();
            } else {
                search_start = abs_end;
            }
        }
    }

    result
}

/// Parsed metric filter reference: metric name + entity list.
struct MetricFilterRef {
    metric_name: String,
    entities: Vec<String>,
}

/// Scan WHERE filters for `{{ Metric('name', ['entity']) }}` and return parsed references.
fn extract_metric_filter_refs(filters: &[String]) -> Vec<MetricFilterRef> {
    let mut refs = Vec::new();
    for filter in filters {
        let mut cursor = 0;
        while let Some(start) = filter[cursor..].find("{{ Metric(") {
            let abs_start = cursor + start;
            let Some(end) = filter[abs_start..].find("}}").map(|i| abs_start + i + 2) else {
                break;
            };
            let inner = &filter[abs_start + 2..end - 2].trim();
            let metric_ref = inner
                .strip_prefix("Metric(")
                .and_then(|s| s.strip_suffix(')'))
                .unwrap_or(inner)
                .trim();

            let metric_name = metric_ref
                .split(',')
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches('\'')
                .trim_matches('"')
                .to_string();

            let entities: Vec<String> = metric_ref
                .split('[')
                .nth(1)
                .and_then(|s| s.split(']').next())
                .map(|s| {
                    s.split(',')
                        .map(|e| e.trim().trim_matches('\'').trim_matches('"').to_string())
                        .filter(|e| !e.is_empty())
                        .collect()
                })
                .unwrap_or_default();

            if !metric_name.is_empty()
                && !refs
                    .iter()
                    .any(|r: &MetricFilterRef| r.metric_name == metric_name)
            {
                refs.push(MetricFilterRef {
                    metric_name,
                    entities,
                });
            }
            cursor = end;
        }
    }
    refs
}

/// Compile metric filter CTEs and return LEFT JOIN clauses to add to the main query.
///
/// For each `{{ Metric('booking_value', ['guest']) }}` reference, produces a CTE like:
///   __mf_booking_value AS (SELECT guest_id AS guest, SUM(booking_value) AS guest__booking_value FROM ... GROUP BY 1)
/// and a JOIN clause like:
///   LEFT JOIN __mf_booking_value ON outer.guest_id = __mf_booking_value.guest
/// Compile a simple metric filter CTE: `SELECT entity_expr AS entity, AGG(expr) AS col FROM src GROUP BY 1`
#[allow(clippy::too_many_arguments)]
fn compile_simple_metric_filter_cte(
    metric_name: &str,
    ap: &AggParams,
    entities: &[String],
    model_aliases: &HashMap<String, (String, &ResolvedModel)>,
    dialect: Dialect,
    ctes: &mut Vec<(String, String)>,
    metric_filters: &[String],
    join_edges: &[JoinEdge],
) -> Option<String> {
    let (source_alias, source_model) = model_aliases.get(&ap.semantic_model)?;
    let cte_name = format!("__mf_{metric_name}");
    let source_relation = render_full_relation(source_model, dialect);
    let agg_expr = render_agg_with_params(
        &ap.agg,
        &ap.expr,
        dialect,
        ap.percentile,
        ap.use_discrete_percentile,
    );

    let mut select_parts: Vec<String> = Vec::new();
    let mut needs_source_alias = false;
    // For multi-hop entity references (e.g. account_id__customer_id__customer_third_hop_id),
    // we need to join through intermediate models and select from the leaf model.
    let mut join_sql = String::new();
    for entity_name in entities {
        let bare_entity = entity_name
            .rsplit("__")
            .next()
            .unwrap_or(entity_name.as_str());
        let source_entity = source_model.entities.iter().find(|e| e.name == bare_entity);
        if source_entity.is_some() {
            let source_expr = source_entity.map(|e| e.expr.as_str()).unwrap();
            select_parts.push(format!("{source_expr} AS {entity_name}"));
        } else if entity_name.contains("__") {
            // Multi-hop: need to find the entity in another model and join to it.
            let segments: Vec<&str> = entity_name.split("__").collect();
            let target_entity = *segments.last().unwrap();
            // Find the model with this entity that is reachable from the source.
            let mut target_model_name = None;
            for (mn, (_a, m)) in model_aliases {
                if mn == &ap.semantic_model {
                    continue;
                }
                let has = m.entities.iter().any(|e| e.name == target_entity);
                if has {
                    target_model_name = Some(mn.clone());
                    break;
                }
            }
            if let Some(ref tmn) = target_model_name {
                if let Some(path) = find_join_path(join_edges, &ap.semantic_model, tmn) {
                    let mut joined_models: HashSet<String> = HashSet::new();
                    joined_models.insert(ap.semantic_model.clone());
                    for edge in &path {
                        if joined_models.contains(&edge.to_model) {
                            continue;
                        }
                        let _left_alias = model_aliases
                            .get(&edge.from_model)
                            .map(|(_, m)| m.name.chars().next().unwrap_or('t'));
                        let (to_alias, to_model) = model_aliases.get(&edge.to_model)?;
                        let to_relation = render_full_relation(to_model, dialect);
                        let fa = model_aliases
                            .get(&edge.from_model)
                            .map(|(a, _)| a.as_str())
                            .unwrap_or("t");
                        if edge.from_model == ap.semantic_model {
                            needs_source_alias = true;
                        }
                        let from_ref = format!("{fa}.{}", edge.from_expr);
                        // Build join condition with partition matching.
                        let mut join_conds =
                            vec![format!("{from_ref} = {to_alias}.{}", edge.to_expr)];
                        if let Some((_, from_model_ref)) = model_aliases.get(&edge.from_model) {
                            for from_dim in &from_model_ref.dimensions {
                                if from_dim.is_partition {
                                    if let Some(to_dim) = to_model
                                        .dimensions
                                        .iter()
                                        .find(|d| d.is_partition && d.name == from_dim.name)
                                    {
                                        join_conds.push(format!(
                                            "DATE_TRUNC('day', CAST({fa}.{} AS TIMESTAMP)) = DATE_TRUNC('day', CAST({to_alias}.{} AS TIMESTAMP))",
                                            from_dim.expr, to_dim.expr
                                        ));
                                    }
                                }
                            }
                        }
                        let _ = write!(
                            join_sql,
                            " LEFT JOIN {to_relation} AS {to_alias} ON ({})",
                            join_conds.join(") AND ("),
                        );
                        joined_models.insert(edge.to_model.clone());
                    }
                    // Select from the target model's entity expression.
                    let (target_alias, target_model) = model_aliases.get(tmn)?;
                    let target_ent = target_model
                        .entities
                        .iter()
                        .find(|e| e.name == target_entity)?;
                    select_parts.push(format!(
                        "{target_alias}.{} AS {entity_name}",
                        target_ent.expr
                    ));
                }
            } else {
                select_parts.push(format!("{entity_name} AS {entity_name}"));
            }
        } else {
            select_parts.push(format!("{entity_name} AS {entity_name}"));
        }
    }

    let col_name = if entities.is_empty() {
        metric_name.to_string()
    } else {
        format!("{}__{metric_name}", entities.join("__"))
    };
    select_parts.push(format!("{agg_expr} AS {col_name}"));

    let group_indices: Vec<String> = (1..=entities.len()).map(|i| i.to_string()).collect();
    let group_clause = if group_indices.is_empty() {
        String::new()
    } else {
        format!(" GROUP BY {}", group_indices.join(", "))
    };

    // Apply metric filters as WHERE clause.
    let where_clause = if metric_filters.is_empty() {
        String::new()
    } else {
        let resolved: Vec<String> = metric_filters
            .iter()
            .map(|f| resolve_where_filter(f, model_aliases, dialect, &ap.semantic_model))
            .collect();
        format!(" WHERE {}", resolved.join(" AND "))
    };

    let from_part = if needs_source_alias {
        format!("{source_relation} AS {source_alias}")
    } else {
        source_relation
    };
    let cte_sql = format!(
        "SELECT {} FROM {from_part}{join_sql}{where_clause}{group_clause}",
        select_parts.join(", "),
    );
    ctes.push((cte_name, cte_sql));
    Some(col_name)
}

#[allow(clippy::cognitive_complexity)]
#[allow(clippy::too_many_arguments)]
fn compile_metric_filter_ctes(
    refs: &[MetricFilterRef],
    all_metrics: &HashMap<String, ResolvedMetric>,
    model_aliases: &HashMap<String, (String, &ResolvedModel)>,
    primary_model_name: &str,
    primary_alias: &str,
    dialect: Dialect,
    ctes: &mut Vec<(String, String)>,
    join_edges: &[JoinEdge],
) -> Vec<String> {
    let mut joins = Vec::new();

    for mfr in refs {
        let metric = match all_metrics.get(&mfr.metric_name) {
            Some(m) => m,
            None => continue,
        };

        let cte_name = format!("__mf_{}", mfr.metric_name);

        match metric.metric_type {
            MetricType::Simple | MetricType::Cumulative => {
                let ap = match &metric.agg_params {
                    Some(ap) => ap,
                    None => continue,
                };
                compile_simple_metric_filter_cte(
                    &mfr.metric_name,
                    ap,
                    &mfr.entities,
                    model_aliases,
                    dialect,
                    ctes,
                    &metric.metric_filters,
                    join_edges,
                );
            }
            MetricType::Derived => {
                // Compile each input metric as a sub-CTE, then build a composite CTE.
                let derived_expr = match &metric.derived_expr {
                    Some(e) => e.clone(),
                    None => continue,
                };
                let mut sub_cols: Vec<(String, String)> = Vec::new(); // (alias/name, cte_col_name)
                for input in &metric.input_metrics {
                    let sub_metric = match all_metrics.get(&input.name) {
                        Some(m) => m,
                        None => continue,
                    };
                    if let Some(sub_ap) = &sub_metric.agg_params {
                        let sub_name = input.alias.as_deref().unwrap_or(&input.name);
                        if let Some(col) = compile_simple_metric_filter_cte(
                            sub_name,
                            sub_ap,
                            &mfr.entities,
                            model_aliases,
                            dialect,
                            ctes,
                            &sub_metric.metric_filters,
                            join_edges,
                        ) {
                            sub_cols.push((sub_name.to_string(), col));
                        }
                    }
                }
                if sub_cols.is_empty() {
                    continue;
                }
                // Build composite CTE joining all sub-CTEs.
                let first_sub = format!("__mf_{}", sub_cols[0].0);
                let mut entity_selects: Vec<String> = Vec::new();
                for entity_name in &mfr.entities {
                    entity_selects.push(format!("{first_sub}.{entity_name}"));
                }
                // Resolve the derived expression: replace alias names with CTE column references.
                // Sort by length (longest first) to prevent substring collisions.
                let mut sorted_subs = sub_cols.clone();
                sorted_subs.sort_by_key(|a| std::cmp::Reverse(a.0.len()));
                let mut resolved_expr = derived_expr.clone();
                for (alias, col) in &sorted_subs {
                    let cte_ref = format!("__mf_{alias}.{col}");
                    resolved_expr = replace_word(&resolved_expr, alias, &cte_ref);
                }
                let out_col = if mfr.entities.is_empty() {
                    mfr.metric_name.clone()
                } else {
                    format!("{}__{}", mfr.entities.join("__"), mfr.metric_name)
                };
                entity_selects.push(format!("{resolved_expr} AS {out_col}"));
                let mut from_clause = first_sub.clone();
                for (alias, _) in sub_cols.iter().skip(1) {
                    let sub_cte = format!("__mf_{alias}");
                    if mfr.entities.is_empty() {
                        from_clause.push_str(&format!(" CROSS JOIN {sub_cte}"));
                    } else {
                        let join_conds: Vec<String> = mfr
                            .entities
                            .iter()
                            .map(|e| format!("{first_sub}.{e} = {sub_cte}.{e}"))
                            .collect();
                        from_clause.push_str(&format!(
                            " LEFT JOIN {sub_cte} ON {}",
                            join_conds.join(" AND ")
                        ));
                    }
                }
                let cte_sql = format!("SELECT {} FROM {from_clause}", entity_selects.join(", "),);
                ctes.push((cte_name.clone(), cte_sql));
            }
            MetricType::Ratio => {
                // Compile numerator and denominator as sub-CTEs.
                let num = match &metric.numerator {
                    Some(n) => n,
                    None => continue,
                };
                let den = match &metric.denominator {
                    Some(d) => d,
                    None => continue,
                };
                let num_metric = match all_metrics.get(&num.name) {
                    Some(m) => m,
                    None => continue,
                };
                let den_metric = match all_metrics.get(&den.name) {
                    Some(m) => m,
                    None => continue,
                };
                let num_ap = match &num_metric.agg_params {
                    Some(ap) => ap,
                    None => continue,
                };
                let den_ap = match &den_metric.agg_params {
                    Some(ap) => ap,
                    None => continue,
                };
                let num_label = format!("{}_num", mfr.metric_name);
                let den_label = format!("{}_den", mfr.metric_name);
                let num_col = compile_simple_metric_filter_cte(
                    &num_label,
                    num_ap,
                    &mfr.entities,
                    model_aliases,
                    dialect,
                    ctes,
                    &num_metric.metric_filters,
                    join_edges,
                );
                let den_col = compile_simple_metric_filter_cte(
                    &den_label,
                    den_ap,
                    &mfr.entities,
                    model_aliases,
                    dialect,
                    ctes,
                    &den_metric.metric_filters,
                    join_edges,
                );
                if let (Some(nc), Some(dc)) = (num_col, den_col) {
                    let num_cte = format!("__mf_{num_label}");
                    let den_cte = format!("__mf_{den_label}");
                    let mut entity_selects: Vec<String> = Vec::new();
                    for entity_name in &mfr.entities {
                        entity_selects.push(format!("{num_cte}.{entity_name}"));
                    }
                    let out_col = if mfr.entities.is_empty() {
                        mfr.metric_name.clone()
                    } else {
                        format!("{}__{}", mfr.entities.join("__"), mfr.metric_name)
                    };
                    let ratio_expr = render_cast_double(&format!("{num_cte}.{nc}"), dialect);
                    let den_expr = format!("NULLIF({den_cte}.{dc}, 0)");
                    entity_selects.push(format!("{ratio_expr} / {den_expr} AS {out_col}"));
                    let join_clause = if mfr.entities.is_empty() {
                        format!("{num_cte} CROSS JOIN {den_cte}")
                    } else {
                        let join_conds: Vec<String> = mfr
                            .entities
                            .iter()
                            .map(|e| format!("{num_cte}.{e} = {den_cte}.{e}"))
                            .collect();
                        format!(
                            "{num_cte} LEFT JOIN {den_cte} ON {}",
                            join_conds.join(" AND ")
                        )
                    };
                    let cte_sql =
                        format!("SELECT {} FROM {join_clause}", entity_selects.join(", "),);
                    ctes.push((cte_name.clone(), cte_sql));
                }
            }
            MetricType::Conversion => continue,
        }

        // Build join condition from the outer CTE to __mf_{metric_name}.
        let mut join_conds: Vec<String> = Vec::new();
        if let Some((_, primary_model)) = model_aliases.get(primary_model_name) {
            for entity_name in &mfr.entities {
                let bare_entity = entity_name
                    .rsplit("__")
                    .next()
                    .unwrap_or(entity_name.as_str());
                let outer_expr = primary_model
                    .entities
                    .iter()
                    .find(|e| e.name == bare_entity)
                    .map(|e| format!("{primary_alias}.{}", e.expr))
                    .unwrap_or_else(|| format!("{primary_alias}.{entity_name}"));
                join_conds.push(format!("{outer_expr} = {cte_name}.{entity_name}"));
            }
        }

        if join_conds.is_empty() {
            joins.push(format!("LEFT JOIN {cte_name} ON TRUE"));
        } else {
            joins.push(format!(
                "LEFT JOIN {cte_name} ON {}",
                join_conds.join(" AND ")
            ));
        }
    }

    joins
}

fn resolve_dimension_ref(
    dim_ref: &str,
    model_aliases: &HashMap<String, (String, &ResolvedModel)>,
    dialect: Dialect,
    primary_model_name: &str,
) -> String {
    resolve_dimension_ref_with_mh(dim_ref, model_aliases, dialect, primary_model_name, &[])
}

fn resolve_dimension_ref_with_mh(
    dim_ref: &str,
    model_aliases: &HashMap<String, (String, &ResolvedModel)>,
    _dialect: Dialect,
    primary_model_name: &str,
    multi_hop_subqueries: &[MultiHopSubquery],
) -> String {
    // For multi-hop paths (e.g. account_id__customer_id__customer_name),
    // use rsplit_once to separate the last segment (dimension) from the
    // entity chain. Then use the last entity in the chain for model lookup.
    let (entity_chain, dim_name) = if let Some((chain, d)) = dim_ref.rsplit_once("__") {
        (Some(chain), d)
    } else {
        (None, dim_ref)
    };

    // Check multi-hop subqueries first — these override normal resolution
    // when dimensions come from different leaf models along the same chain.
    if !multi_hop_subqueries.is_empty() {
        for mh in multi_hop_subqueries {
            if let Some(col_expr) = mh.dim_columns.get(dim_name) {
                return col_expr.clone();
            }
        }
    }

    let target_entity =
        entity_chain.map(|chain| chain.rsplit_once("__").map_or(chain, |(_, last)| last));

    let check_model = |alias: &str, model: &ResolvedModel| -> Option<String> {
        if let Some(entity_name) = target_entity {
            let has_entity = model.entities.iter().any(|e| e.name == entity_name)
                || model.primary_entity.as_deref() == Some(entity_name);
            if !has_entity {
                return None;
            }
        }
        if let Some(dim) = model.dimensions.iter().find(|d| d.name == dim_name) {
            return Some(format!("{}.{}", alias, dim.expr));
        }
        // Also resolve entities used as dimensions (dundered identifiers).
        if let Some(ent) = model.entities.iter().find(|e| e.name == dim_name) {
            return Some(format!("{}.{}", alias, ent.expr));
        }
        None
    };

    // For entity-prefixed dimensions, prefer the model where the entity
    // is primary/unique (the canonical source).
    if let Some(entity_name) = target_entity {
        for (alias, model) in model_aliases.values() {
            let is_primary = model.entities.iter().any(|e| {
                e.name == entity_name
                    && matches!(e.entity_type.as_str(), "primary" | "unique" | "natural")
            }) || model.primary_entity.as_deref() == Some(entity_name);
            if is_primary {
                if let Some(result) = check_model(alias, model) {
                    return result;
                }
            }
        }
    }
    // Check primary model first for deterministic resolution.
    if let Some((alias, model)) = model_aliases.get(primary_model_name) {
        if let Some(result) = check_model(alias, model) {
            return result;
        }
    }
    for (alias, model) in model_aliases.values() {
        if let Some(result) = check_model(alias, model) {
            return result;
        }
    }

    // Fallback: use the dimension name as-is.
    dim_name.to_string()
}

/// Resolve `Entity('name')` to `alias.expr` by finding the entity in any resolved model.
/// Checks the primary model first for deterministic resolution.
fn resolve_entity_ref(
    entity_name: &str,
    model_aliases: &HashMap<String, (String, &ResolvedModel)>,
    primary_model_name: &str,
) -> String {
    // Check the primary model first.
    if let Some((alias, model)) = model_aliases.get(primary_model_name) {
        if let Some(ent) = model.entities.iter().find(|e| e.name == entity_name) {
            return format!("{}.{}", alias, ent.expr);
        }
    }
    // Check remaining models.
    for (alias, model) in model_aliases.values() {
        if let Some(ent) = model.entities.iter().find(|e| e.name == entity_name) {
            return format!("{}.{}", alias, ent.expr);
        }
    }
    // Fallback: use entity name as column name.
    entity_name.to_string()
}

fn resolve_raw_time_column(
    name: &str,
    model_aliases: &HashMap<String, (String, &ResolvedModel)>,
    primary_model_name: &str,
) -> String {
    let dim_name = name.split_once("__").map_or(name, |(_, d)| d);
    let check = |alias: &str, model: &ResolvedModel| -> Option<String> {
        for dim in &model.dimensions {
            if dim.dimension_type == "time"
                && (name == "metric_time" || dim.name == dim_name || dim.name == name)
            {
                return Some(format!("{}.{}", alias, dim.expr));
            }
        }
        None
    };
    if let Some((alias, model)) = model_aliases.get(primary_model_name) {
        if let Some(result) = check(alias, model) {
            return result;
        }
    }
    for (alias, model) in model_aliases.values() {
        if let Some(result) = check(alias, model) {
            return result;
        }
    }
    name.to_string()
}

fn resolve_time_dimension_ref(
    name: &str,
    granularity: &str,
    model_aliases: &HashMap<String, (String, &ResolvedModel)>,
    dialect: Dialect,
    primary_model_name: &str,
) -> String {
    resolve_time_dimension_ref_with_agg(
        name,
        granularity,
        model_aliases,
        dialect,
        primary_model_name,
        None,
    )
}

fn resolve_time_dimension_ref_with_agg(
    name: &str,
    granularity: &str,
    model_aliases: &HashMap<String, (String, &ResolvedModel)>,
    dialect: Dialect,
    primary_model_name: &str,
    agg_time_dim: Option<&str>,
) -> String {
    // Strip an optional entity prefix: `user_account_activity__date_day` → `date_day`.
    // Use rsplit_once to correctly handle multi-hop chains:
    // `listing__user__ds` → entity_prefix = "listing__user", dim_name = "ds".
    let (entity_prefix, dim_name) = match name.rsplit_once("__") {
        Some((e, d)) => (Some(e), d),
        None => (None, name),
    };
    // For multi-hop chains, the target entity is the last segment of the chain.
    let target_entity = entity_prefix.and_then(|e| {
        if e.contains("__") {
            e.rsplit_once("__").map(|(_, last)| last)
        } else {
            Some(e)
        }
    });

    let check_model = |alias: &str,
                       model: &ResolvedModel,
                       require_primary_entity: bool,
                       allow_any_time_dim: bool|
     -> Option<String> {
        if let Some(entity) = target_entity.or(entity_prefix) {
            let entity_entry = model.entities.iter().find(|e| e.name == entity);
            let is_primary_entity = model.primary_entity.as_deref() == Some(entity);
            let has_entity = entity_entry.is_some() || is_primary_entity;
            if !has_entity {
                return None;
            }
            if require_primary_entity {
                let is_primary_or_unique = is_primary_entity
                    || entity_entry.is_some_and(|e| {
                        matches!(e.entity_type.as_str(), "primary" | "unique" | "natural")
                    });
                if !is_primary_or_unique {
                    return None;
                }
            }
        }
        if name == "metric_time" {
            if let Some(atd) = agg_time_dim {
                for dim in &model.dimensions {
                    if dim.dimension_type == "time" && dim.name == atd {
                        return Some(render_date_trunc(
                            granularity,
                            &format!("{}.{}", alias, dim.expr),
                            dialect,
                        ));
                    }
                }
            }
        }
        for dim in &model.dimensions {
            if dim.dimension_type == "time"
                && (name == "metric_time" || dim.name == dim_name || dim.name == name)
            {
                return Some(render_date_trunc(
                    granularity,
                    &format!("{}.{}", alias, dim.expr),
                    dialect,
                ));
            }
        }
        // When entity-prefixed, fall back to any time dimension on the target model.
        if allow_any_time_dim && entity_prefix.is_some() && name != "metric_time" {
            for dim in &model.dimensions {
                if dim.dimension_type == "time" {
                    return Some(render_date_trunc(
                        granularity,
                        &format!("{}.{}", alias, dim.expr),
                        dialect,
                    ));
                }
            }
        }
        None
    };

    // When entity-prefixed, prefer the model where the entity is primary/unique.
    // First pass: exact dim name match only.
    if entity_prefix.is_some() {
        for (alias, model) in model_aliases.values() {
            if let Some(result) = check_model(alias, model, true, false) {
                return result;
            }
        }
        // Second pass: allow any time dim fallback.
        for (alias, model) in model_aliases.values() {
            if let Some(result) = check_model(alias, model, true, true) {
                return result;
            }
        }
    }
    // Check primary model first for deterministic resolution.
    if let Some((alias, model)) = model_aliases.get(primary_model_name) {
        if let Some(result) = check_model(alias, model, false, false) {
            return result;
        }
    }
    for (alias, model) in model_aliases.values() {
        if let Some(result) = check_model(alias, model, false, false) {
            return result;
        }
    }
    for (alias, model) in model_aliases.values() {
        if let Some(result) = check_model(alias, model, false, true) {
            return result;
        }
    }

    // Fallback.
    render_date_trunc(granularity, name, dialect)
}

// ═══════════════════════════════════════════════════════════════════════════
// Dialect-specific SQL rendering helpers
// ═══════════════════════════════════════════════════════════════════════════

fn render_type_cast(expr: &str, sql_type: &str, dialect: Dialect) -> String {
    match dialect {
        Dialect::DuckDB | Dialect::Snowflake | Dialect::Redshift => {
            format!("{expr}::{sql_type}")
        }
        Dialect::BigQuery | Dialect::Databricks => {
            format!("CAST({expr} AS {sql_type})")
        }
    }
}

fn render_date_trunc(granularity: &str, expr: &str, dialect: Dialect) -> String {
    let subdaily = matches!(granularity, "hour" | "minute" | "second" | "millisecond");
    let target_type = if subdaily { "TIMESTAMP" } else { "DATE" };
    let gran_upper = granularity.to_uppercase();
    let raw = match dialect {
        Dialect::BigQuery => format!("DATE_TRUNC({expr}, {gran_upper})"),
        _ => format!("DATE_TRUNC('{granularity}', {expr})"),
    };
    render_type_cast(&raw, target_type, dialect)
}

fn render_extract(part: &str, expr: &str, dialect: Dialect) -> String {
    let p = part.to_uppercase();
    if p == "DOW" {
        return match dialect {
            Dialect::DuckDB => format!("EXTRACT(ISODOW FROM {expr})"),
            Dialect::Snowflake => format!("EXTRACT(DAYOFWEEKISO FROM {expr})"),
            Dialect::Databricks => format!("EXTRACT(DAYOFWEEK_ISO FROM {expr})"),
            Dialect::Redshift => {
                format!("(EXTRACT(DOW FROM {expr}) + 6) % 7 + 1")
            }
            Dialect::BigQuery => {
                let base = format!("EXTRACT(DAYOFWEEK FROM {expr})");
                format!("IF({base} = 1, 7, {base} - 1)")
            }
        };
    }
    let mapped = match p.as_str() {
        "DOY" if matches!(dialect, Dialect::BigQuery) => "DAYOFYEAR",
        other => other,
    };
    format!("EXTRACT({mapped} FROM {expr})")
}

fn render_cast_double(expr: &str, dialect: Dialect) -> String {
    let float_type = match dialect {
        Dialect::DuckDB | Dialect::Databricks => "DOUBLE",
        Dialect::Snowflake => "FLOAT",
        Dialect::Redshift => "DOUBLE PRECISION",
        Dialect::BigQuery => "FLOAT64",
    };
    format!("CAST({expr} AS {float_type})")
}

fn render_interval(count: i64, granularity: &str, dialect: Dialect) -> String {
    let gran_upper = granularity.to_uppercase();
    match dialect {
        Dialect::Redshift => {
            let (days, gran) = match granularity {
                "month" | "months" => (count * 30, "day"),
                "year" | "years" => (count * 365, "day"),
                "week" | "weeks" => (count * 7, "day"),
                _ => (count, granularity),
            };
            format!("INTERVAL '{days} {gran}'")
        }
        Dialect::DuckDB | Dialect::Snowflake => {
            format!("INTERVAL '{count} {granularity}'")
        }
        Dialect::BigQuery | Dialect::Databricks => {
            format!("INTERVAL {count} {gran_upper}")
        }
    }
}

fn render_interval_str(raw: &str, dialect: Dialect) -> String {
    if matches!(dialect, Dialect::Redshift) {
        if let Some((num_str, gran)) = raw.split_once(' ') {
            if let Ok(count) = num_str.trim().parse::<i64>() {
                return render_interval(count, gran.trim(), dialect);
            }
        }
    }
    format!("INTERVAL '{raw}'")
}

/// Resolve an order-by name to the canonical SQL output column name.
///
/// When multiple granularities of the same time dimension are present, the
/// output columns are qualified (`metric_time__month`), so we return the
/// qualified form.  Otherwise the bare name is used.
fn resolve_order_by_col(name: &str, group_by: &[GroupBySpec]) -> String {
    let out_cols = group_by_output_cols(group_by);
    // Direct match on output column names.
    if out_cols.iter().any(|c| c == name) {
        return name.to_string();
    }
    // Match `metric_time__month` against a TimeDimension with that qualified name.
    for (gb, out_col) in group_by.iter().zip(out_cols.iter()) {
        if let GroupBySpec::TimeDimension {
            name: td_name,
            granularity,
            ..
        } = gb
        {
            if name == format!("{td_name}__{granularity}") {
                return out_col.clone();
            }
        }
    }
    name.to_string()
}

#[cfg(test)]
fn render_agg(agg: &str, expr: &str, dialect: Dialect) -> String {
    render_agg_with_params(agg, expr, dialect, None, false)
}

fn render_agg_with_params(
    agg: &str,
    expr: &str,
    dialect: Dialect,
    percentile: Option<f64>,
    use_discrete: bool,
) -> String {
    match agg {
        "sum" => format!("SUM({expr})"),
        "count" => format!("COUNT({expr})"),
        "count_distinct" => format!("COUNT(DISTINCT {expr})"),
        "average" | "avg" => format!("AVG({expr})"),
        "min" => format!("MIN({expr})"),
        "max" => format!("MAX({expr})"),
        "sum_boolean" => match dialect {
            Dialect::DuckDB => format!("SUM(CAST({expr} AS INTEGER))"),
            _ => format!("SUM(CASE WHEN {expr} THEN 1 ELSE 0 END)"),
        },
        "median" => match dialect {
            Dialect::BigQuery => format!("APPROX_QUANTILES({expr}, 2)[OFFSET(1)]"),
            _ => format!("MEDIAN({expr})"),
        },
        "percentile" => {
            let pct = percentile.unwrap_or(0.5);
            if use_discrete {
                match dialect {
                    Dialect::BigQuery => {
                        let offset = (pct * 100.0) as i64;
                        format!("APPROX_QUANTILES({expr}, 100)[OFFSET({offset})]")
                    }
                    _ => format!("PERCENTILE_DISC({pct}) WITHIN GROUP (ORDER BY {expr})"),
                }
            } else {
                match dialect {
                    Dialect::BigQuery => {
                        let offset = (pct * 100.0) as i64;
                        format!("APPROX_QUANTILES({expr}, 100)[OFFSET({offset})]")
                    }
                    _ => format!("PERCENTILE_CONT({pct}) WITHIN GROUP (ORDER BY {expr})"),
                }
            }
        }
        "approximate_continuous" | "approximate_discrete" => {
            let pct = percentile.unwrap_or(0.5);
            match dialect {
                Dialect::BigQuery => {
                    let offset = (pct * 100.0) as i64;
                    format!("APPROX_QUANTILES({expr}, 100)[OFFSET({offset})]")
                }
                Dialect::DuckDB => format!("APPROX_QUANTILE({expr}, {pct})"),
                _ => format!("APPROX_PERCENTILE({expr}, {pct})"),
            }
        }
        other => format!("{other}({expr})"),
    }
}

/// Qualify a measure expression with a table alias.
/// Literals (numbers, `*`, strings) and expressions containing operators or
/// function calls are left as-is; simple column names get `alias.col`.
/// Word-boundary-aware string replacement.  Only replaces occurrences of
/// `find` that are not immediately preceded or followed by an identifier
/// character (alphanumeric or underscore).
fn replace_word(text: &str, find: &str, replace: &str) -> String {
    if find.is_empty() {
        return text.to_string();
    }
    let mut result = String::with_capacity(text.len());
    let text_bytes = text.as_bytes();
    let find_bytes = find.as_bytes();
    let mut i = 0;
    while i <= text_bytes.len().saturating_sub(find_bytes.len()) {
        if text_bytes[i..].starts_with(find_bytes) {
            // Check character before match.
            let before_ok = i == 0 || {
                let c = text_bytes[i - 1];
                !c.is_ascii_alphanumeric() && c != b'_'
            };
            // Check character after match.
            let after_pos = i + find_bytes.len();
            let after_ok = after_pos >= text_bytes.len() || {
                let c = text_bytes[after_pos];
                !c.is_ascii_alphanumeric() && c != b'_'
            };
            if before_ok && after_ok {
                result.push_str(replace);
                i += find_bytes.len();
                continue;
            }
        }
        result.push(text_bytes[i] as char);
        i += 1;
    }
    // Append remaining characters that couldn't start a match.
    while i < text_bytes.len() {
        result.push(text_bytes[i] as char);
        i += 1;
    }
    result
}

/// Returns `true` if `expr` is a compound expression — one that cannot be
/// trivially qualified by prepending a table alias — because it contains
/// operators, function calls, or SQL keywords (e.g. a CASE expression).
///
/// Pure literals (`*`, numeric constants, string literals) return `false`
/// because they contain no column references and are never ambiguous.
fn is_complex_measure_expr(expr: &str) -> bool {
    let trimmed = expr.trim();
    if trimmed == "*" || trimmed.parse::<f64>().is_ok() || trimmed.starts_with('\'') {
        return false;
    }
    trimmed.contains('(') || trimmed.contains(' ')
}

fn qualify_measure_expr(alias: &str, expr: &str) -> String {
    let trimmed = expr.trim();
    if trimmed == "*"
        || trimmed.parse::<f64>().is_ok()
        || trimmed.starts_with('\'')
        || is_complex_measure_expr(trimmed)
    {
        trimmed.to_string()
    } else {
        format!("{alias}.{trimmed}")
    }
}

fn render_full_relation(model: &ResolvedModel, dialect: Dialect) -> String {
    match dialect {
        Dialect::BigQuery | Dialect::Databricks => model.relation_name.replace('"', "`"),
        _ => model.relation_name.clone(),
    }
}

/// Generate an inline time spine CTE that produces a DATE column named `out_col`
/// spanning `[MIN(src_col) FROM src_cte .. MAX(src_col) FROM src_cte]` at the given granularity.
///
/// `src_col` is the column in the source CTE to derive the range from.
/// `out_col` is the output column name in the spine CTE.
///
/// DuckDB:    `SELECT ds::DATE AS out_col FROM generate_series(MIN, MAX, INTERVAL '1 gran') AS t(ds)`
/// Snowflake: `SELECT DATEADD(gran, ROW_NUMBER() OVER (ORDER BY 1) - 1, MIN)::DATE AS out_col
///             FROM TABLE(GENERATOR(ROWCOUNT => DATEDIFF(gran, MIN, MAX) + 1))`
fn inline_time_spine_sql(
    out_col: &str,
    src_cte: &str,
    src_col: &str,
    granularity: &str,
    dialect: Dialect,
) -> String {
    inline_time_spine_sql_bounded(out_col, src_cte, src_col, granularity, dialect, None)
}

fn inline_time_spine_sql_bounded(
    out_col: &str,
    src_cte: &str,
    src_col: &str,
    granularity: &str,
    dialect: Dialect,
    max_bound: Option<&str>,
) -> String {
    let min_expr = format!("(SELECT MIN({src_col}) FROM {src_cte})");
    let max_expr = if let Some(bound) = max_bound {
        format!("GREATEST((SELECT MAX({src_col}) FROM {src_cte}), CAST('{bound}' AS DATE))")
    } else {
        format!("(SELECT MAX({src_col}) FROM {src_cte})")
    };
    let subdaily = matches!(granularity, "hour" | "minute" | "second" | "millisecond");
    let target_type = if subdaily { "TIMESTAMP" } else { "DATE" };
    let gran_upper = granularity.to_uppercase();
    match dialect {
        Dialect::DuckDB => {
            let cast = render_type_cast("ds", target_type, dialect);
            format!(
                "SELECT {cast} AS {out_col} \
                 FROM generate_series({min_expr}, {max_expr}, INTERVAL '1 {granularity}') AS t(ds)"
            )
        }
        Dialect::Snowflake => {
            let dateadd =
                format!("DATEADD('{granularity}', ROW_NUMBER() OVER (ORDER BY 1) - 1, {min_expr})");
            let cast = render_type_cast(&dateadd, target_type, dialect);
            format!(
                "SELECT {cast} AS {out_col} \
                 FROM TABLE(GENERATOR(ROWCOUNT => 100000)) \
                 QUALIFY {out_col} <= {max_expr}"
            )
        }
        Dialect::Redshift => {
            let digits = "(SELECT 0 n UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 \
                           UNION ALL SELECT 4 UNION ALL SELECT 5 UNION ALL SELECT 6 \
                           UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9)";
            let dateadd = format!("DATEADD('{granularity}', n, {min_expr})");
            let cast = render_type_cast(&dateadd, target_type, dialect);
            format!(
                "SELECT {cast} AS {out_col} \
                 FROM (\
                 SELECT (p0.n + p1.n * 10 + p2.n * 100 + p3.n * 1000 + p4.n * 10000) AS n \
                 FROM {digits} p0 \
                 CROSS JOIN {digits} p1 \
                 CROSS JOIN {digits} p2 \
                 CROSS JOIN {digits} p3 \
                 CROSS JOIN {digits} p4\
                 ) \
                 WHERE {out_col} <= {max_expr}"
            )
        }
        Dialect::BigQuery => {
            let array_fn = if subdaily {
                format!("GENERATE_TIMESTAMP_ARRAY({min_expr}, {max_expr}, INTERVAL 1 {gran_upper})")
            } else {
                format!("GENERATE_DATE_ARRAY({min_expr}, {max_expr}, INTERVAL 1 {gran_upper})")
            };
            format!("SELECT {out_col} FROM UNNEST({array_fn}) AS {out_col}")
        }
        Dialect::Databricks => {
            format!(
                "SELECT EXPLODE(SEQUENCE({min_expr}, {max_expr}, INTERVAL 1 {gran_upper})) AS {out_col}"
            )
        }
    }
}

fn expand_time_constraint_to_granularity(
    start: &str,
    end: &str,
    granularity: &str,
) -> (String, String) {
    fn parse_date(s: &str) -> Option<(i32, u32, u32)> {
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() != 3 {
            return None;
        }
        Some((
            parts[0].parse().ok()?,
            parts[1].parse().ok()?,
            parts[2].parse().ok()?,
        ))
    }

    fn days_in_month(year: i32, month: u32) -> u32 {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                    29
                } else {
                    28
                }
            }
            _ => 30,
        }
    }

    fn day_of_week(year: i32, month: u32, day: u32) -> u32 {
        // Zeller-like: 0=Mon, 6=Sun
        let (y, m) = if month <= 2 {
            (year - 1, month + 12)
        } else {
            (year, month)
        };
        let q = day as i32;
        let k = y % 100;
        let j = y / 100;
        let h = (q + (13 * (m as i32 + 1)) / 5 + k + k / 4 + j / 4 + 5 * j) % 7;
        // Convert Zeller's h (0=Sat) to ISO (0=Mon)
        ((h + 5) % 7) as u32
    }

    fn format_date(year: i32, month: u32, day: u32) -> String {
        format!("{year:04}-{month:02}-{day:02}")
    }

    let rank = granularity_rank(granularity);
    if rank <= 7 {
        // day or finer — no expansion needed
        return (start.to_string(), end.to_string());
    }

    let Some((sy, sm, sd)) = parse_date(start) else {
        return (start.to_string(), end.to_string());
    };
    let Some((ey, em, ed)) = parse_date(end) else {
        return (start.to_string(), end.to_string());
    };

    let new_start = match granularity {
        "week" => {
            let dow = day_of_week(sy, sm, sd);
            // Go back to Monday
            let mut d = sd as i32 - dow as i32;
            let (mut y, mut m) = (sy, sm);
            if d < 1 {
                // Previous month
                if m == 1 {
                    m = 12;
                    y -= 1;
                } else {
                    m -= 1;
                }
                d += days_in_month(y, m) as i32;
            }
            format_date(y, m, d as u32)
        }
        "month" => format_date(sy, sm, 1),
        "quarter" => {
            let qm = ((sm - 1) / 3) * 3 + 1;
            format_date(sy, qm, 1)
        }
        "year" => format_date(sy, 1, 1),
        _ => start.to_string(),
    };

    let new_end = match granularity {
        "week" => {
            let dow = day_of_week(ey, em, ed);
            // Go forward to Sunday (6) then to next Monday minus 1 day = Sunday
            let days_to_sunday = if dow == 0 { 6 } else { 6 - dow };
            let mut d = ed + days_to_sunday;
            let (mut y, mut m) = (ey, em);
            let dim = days_in_month(y, m);
            if d > dim {
                d -= dim;
                m += 1;
                if m > 12 {
                    m = 1;
                    y += 1;
                }
            }
            format_date(y, m, d)
        }
        "month" => {
            // End of the month containing end date
            let dim = days_in_month(ey, em);
            format_date(ey, em, dim)
        }
        "quarter" => {
            let qm = ((em - 1) / 3) * 3 + 3;
            let dim = days_in_month(ey, qm);
            format_date(ey, qm, dim)
        }
        "year" => format_date(ey, 12, 31),
        _ => end.to_string(),
    };

    (new_start, new_end)
}

// ═══════════════════════════════════════════════════════════════════════════
// Main compilation pipeline
// ═══════════════════════════════════════════════════════════════════════════

/// Compile a semantic query spec into SQL.
#[allow(clippy::cognitive_complexity)]
pub fn compile(
    store: &mut impl MetricStore,
    spec: &SemanticQuerySpec,
    dialect: Dialect,
) -> Result<String, MetricFlowError> {
    // 1. Resolve all metrics (recursively for derived/ratio).
    let mut all_metrics: HashMap<String, ResolvedMetric> = HashMap::new();
    for metric_name in &spec.metrics {
        resolve_metrics_recursive(store, metric_name, &mut all_metrics)?;
    }

    // Expand time constraints to granularity boundaries, considering both query
    // granularity and metric-level time_granularity.
    let spec = &{
        let mut s = spec.clone();
        // Find the coarsest granularity from query group_by (metric_time only)
        // AND metric-level time_granularity.
        let coarsest_query = s
            .group_by
            .iter()
            .filter_map(|gb| {
                if let GroupBySpec::TimeDimension {
                    name, granularity, ..
                } = gb
                {
                    if name == "metric_time" {
                        Some(granularity.as_str())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .max_by_key(|g| granularity_rank(g))
            .unwrap_or("day");

        let coarsest_metric = s
            .metrics
            .iter()
            .filter_map(|name| all_metrics.get(name))
            .filter_map(|m| m.time_granularity.as_deref())
            .max_by_key(|g| granularity_rank(g))
            .unwrap_or("day");

        let coarsest = if granularity_rank(coarsest_metric) > granularity_rank(coarsest_query) {
            coarsest_metric
        } else {
            coarsest_query
        };

        if let Some((ref start, ref end)) = s.time_constraint {
            let (new_start, new_end) = expand_time_constraint_to_granularity(start, end, coarsest);
            s.time_constraint = Some((new_start, new_end));
        }

        // Also clamp group-by granularities: if a metric's time_granularity is coarser
        // than the query granularity, bump metric_time to at least the metric's grain.
        // Only applies to metric_time, not entity-prefixed time dimensions.
        if s.metrics.len() == 1 {
            if let Some(metric_gran) = all_metrics
                .get(&s.metrics[0])
                .and_then(|m| m.time_granularity.as_deref())
            {
                for gb in &mut s.group_by {
                    if let GroupBySpec::TimeDimension {
                        name, granularity, ..
                    } = gb
                    {
                        if name == "metric_time"
                            && granularity_rank(metric_gran) > granularity_rank(granularity)
                        {
                            *granularity = metric_gran.to_string();
                        }
                    }
                }
            }
        }

        s
    };

    // 1b. Also resolve metrics referenced in {{ Metric() }} WHERE filters.
    let all_filters: Vec<String> = all_metrics
        .values()
        .flat_map(|m| m.metric_filters.iter().cloned())
        .chain(spec.where_filters.iter().cloned())
        .collect();
    for mfr in extract_metric_filter_refs(&all_filters) {
        resolve_metrics_recursive(store, &mfr.metric_name, &mut all_metrics)?;
    }

    // 2. Identify which metric types we're dealing with.
    let top_level: Vec<&ResolvedMetric> = spec
        .metrics
        .iter()
        .filter_map(|name| all_metrics.get(name))
        .collect();

    // 3. Compile based on the metric types present.
    // If all top-level metrics are simple, we can often combine into one query.
    // Derived and ratio metrics require subquery composition.

    let mut ctes: Vec<(String, String)> = Vec::new();
    let mut final_select_columns: Vec<String> = Vec::new();
    let mut final_from = String::new();
    let mut final_joins: Vec<String> = Vec::new();

    // Collect all needed semantic models and build the join graph.
    let join_edges = build_join_graph(store)?;

    // Determine which models are needed from metrics.
    let mut needed_models = collect_needed_models(&all_metrics, &spec.metrics)?;

    // Also resolve models needed for dimension entity references in group_by.
    for gb in &spec.group_by {
        match gb {
            GroupBySpec::Dimension {
                entity: Some(entity_name),
                name,
            } => {
                // For multi-hop entity chains (e.g. "account_id__customer_id"),
                // resolve all intermediate and target models.
                let entity_segments: Vec<&str> = entity_name.split("__").collect();
                if entity_segments.len() > 1 {
                    // Add all models that participate in the entity chain.
                    // Use lookup_all_join_graph_entities to find bridge models.
                    let join_rows = store.lookup_all_join_graph_entities()?;
                    for seg in &entity_segments {
                        for row in &join_rows {
                            if row.entity_name == *seg && !needed_models.contains(&row.model_name) {
                                needed_models.push(row.model_name.clone());
                            }
                        }
                    }
                }
                let target_entity = entity_segments.last().copied().unwrap_or(entity_name);
                if let Some(model_name) = find_model_for_entity_and_dim(store, target_entity, name)?
                {
                    if !needed_models.contains(&model_name) {
                        needed_models.push(model_name);
                    }
                }
                // Single-hop fallback
                if entity_segments.len() == 1 {
                    if let Some(model_name) =
                        find_model_for_entity_and_dim(store, entity_name, name)?
                    {
                        if !needed_models.contains(&model_name) {
                            needed_models.push(model_name);
                        }
                    }
                }
            }
            GroupBySpec::TimeDimension { name, .. } => {
                if let Some((entity_chain, dim_name)) = name.rsplit_once("__") {
                    let entity_segments: Vec<&str> = entity_chain.split("__").collect();
                    if entity_segments.len() > 1 {
                        let join_rows = store.lookup_all_join_graph_entities()?;
                        for seg in &entity_segments {
                            for row in &join_rows {
                                if row.entity_name == *seg
                                    && !needed_models.contains(&row.model_name)
                                {
                                    needed_models.push(row.model_name.clone());
                                }
                            }
                        }
                    }
                    let target_entity = entity_segments.last().copied().unwrap_or(entity_chain);
                    if let Some(model_name) =
                        find_model_for_entity_and_dim(store, target_entity, dim_name)?
                    {
                        if !needed_models.contains(&model_name) {
                            needed_models.push(model_name);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Also resolve models needed for Entity group-by specs — but only if
    // no already-needed model provides the entity.
    for gb in &spec.group_by {
        if let GroupBySpec::Entity { name: entity_name } = gb {
            let already_covered = needed_models.iter().any(|mn| {
                store
                    .check_entity_in_model(mn, entity_name)
                    .unwrap_or(false)
            });
            if !already_covered {
                let model_name = find_model_for_entity_pk(store, entity_name)?;
                let model_name = if model_name.is_some() {
                    model_name
                } else {
                    find_model_for_entity_any(store, entity_name)?
                };
                if let Some(model_name) = model_name {
                    if !needed_models.contains(&model_name) {
                        needed_models.push(model_name);
                    }
                }
            }
        }
    }

    // Also resolve models needed for entity references in where filters.
    // e.g. {{ Dimension('listing__country_latest') }} needs the listing model.
    // e.g. {{ TimeDimension('listing__ds', 'alien_day') }} needs the listing model.
    for filter in spec
        .where_filters
        .iter()
        .chain(all_metrics.values().flat_map(|m| m.metric_filters.iter()))
    {
        let mut cursor = 0usize;
        while let Some(dim_start) = filter[cursor..].find("Dimension(") {
            let abs_pos = cursor + dim_start;
            // Skip if this is actually "TimeDimension(".
            if abs_pos > 0 && filter.as_bytes()[abs_pos - 1].is_ascii_alphanumeric() {
                cursor = abs_pos + 10;
                continue;
            }
            let abs_start = abs_pos + 10;
            if let Some(paren_end) = filter[abs_start..].find(')') {
                let dim_ref = filter[abs_start..abs_start + paren_end]
                    .trim()
                    .trim_matches('\'')
                    .trim_matches('"');
                if let Some((entity_name, _)) = dim_ref.split_once("__") {
                    if let Some(model_name) = find_model_for_entity_pk(store, entity_name)? {
                        if !needed_models.contains(&model_name) {
                            needed_models.push(model_name);
                        }
                    }
                }
                cursor = abs_start + paren_end + 1;
            } else {
                break;
            }
        }
        // Also extract entity references from TimeDimension patterns.
        let mut cursor = 0usize;
        while let Some(td_start) = filter[cursor..].find("TimeDimension(") {
            let abs_start = cursor + td_start + 14;
            if let Some(paren_end) = filter[abs_start..].find(')') {
                let inner = &filter[abs_start..abs_start + paren_end];
                let td_name = inner
                    .split(',')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_matches('\'')
                    .trim_matches('"');
                if let Some((entity_name, _)) = td_name.split_once("__") {
                    if let Some(model_name) = find_model_for_entity_pk(store, entity_name)? {
                        if !needed_models.contains(&model_name) {
                            needed_models.push(model_name);
                        }
                    }
                }
                cursor = abs_start + paren_end + 1;
            } else {
                break;
            }
        }
    }

    // Also resolve models needed for per-input metric filters.
    // Ratio metrics have numerator/denominator filters; derived metrics have
    // per-input filters. These reference dimensions on joined models.
    for m in all_metrics.values() {
        let input_filters = m
            .input_metrics
            .iter()
            .flat_map(|i| i.filters.iter())
            .chain(m.numerator.iter().flat_map(|n| n.filters.iter()))
            .chain(m.denominator.iter().flat_map(|d| d.filters.iter()));
        for filter in input_filters {
            let mut cursor = 0usize;
            while let Some(dim_start) = filter[cursor..].find("Dimension(") {
                let abs_pos = cursor + dim_start;
                if abs_pos > 0 && filter.as_bytes()[abs_pos - 1].is_ascii_alphanumeric() {
                    cursor = abs_pos + 10;
                    continue;
                }
                let abs_start = abs_pos + 10;
                if let Some(paren_end) = filter[abs_start..].find(')') {
                    let dim_ref = filter[abs_start..abs_start + paren_end]
                        .trim()
                        .trim_matches('\'')
                        .trim_matches('"');
                    if let Some((entity_name, _)) = dim_ref.split_once("__") {
                        if let Some(model_name) = find_model_for_entity_pk(store, entity_name)? {
                            if !needed_models.contains(&model_name) {
                                needed_models.push(model_name);
                            }
                        }
                    }
                    cursor = abs_start + paren_end + 1;
                } else {
                    break;
                }
            }
        }
    }

    // Also resolve models needed for {{ Metric(...) }} filter references.
    // The metric filter may reference metrics from models not otherwise needed.
    // For derived/ratio metrics, we need models from their input metrics too.
    for mfr in &extract_metric_filter_refs(&all_filters) {
        if let Some(m) = all_metrics.get(&mfr.metric_name) {
            if let Some(ref ap) = m.agg_params {
                if !ap.semantic_model.is_empty() && !needed_models.contains(&ap.semantic_model) {
                    needed_models.push(ap.semantic_model.clone());
                }
            }
            // Derived: resolve models for each input metric.
            for input in &m.input_metrics {
                if let Some(sub) = all_metrics.get(&input.name) {
                    if let Some(ref ap) = sub.agg_params {
                        if !ap.semantic_model.is_empty()
                            && !needed_models.contains(&ap.semantic_model)
                        {
                            needed_models.push(ap.semantic_model.clone());
                        }
                    }
                }
            }
            // Ratio: resolve models for numerator and denominator.
            for mi in m.numerator.iter().chain(m.denominator.iter()) {
                if let Some(sub) = all_metrics.get(&mi.name) {
                    if let Some(ref ap) = sub.agg_params {
                        if !ap.semantic_model.is_empty()
                            && !needed_models.contains(&ap.semantic_model)
                        {
                            needed_models.push(ap.semantic_model.clone());
                        }
                    }
                }
            }
        }
    }

    // Discover intermediate models needed for multi-hop join paths.
    // For each target model, find the BFS path from the primary model and
    // add any intermediate models that aren't yet in needed_models.
    let primary_model_name = needed_models.first().cloned().unwrap_or_default();
    let mut extra_models: Vec<String> = Vec::new();
    for target in &needed_models {
        if *target == primary_model_name {
            continue;
        }
        if let Some(path) = find_join_path(&join_edges, &primary_model_name, target) {
            for edge in &path {
                if !needed_models.contains(&edge.to_model) && !extra_models.contains(&edge.to_model)
                {
                    extra_models.push(edge.to_model.clone());
                }
            }
        }
    }
    needed_models.extend(extra_models);

    let mut resolved_models: HashMap<String, ResolvedModel> = HashMap::new();
    for model_name in &needed_models {
        resolved_models.insert(model_name.clone(), resolve_model(store, model_name)?);
    }

    // Assign table aliases.  Use the deterministic `needed_models` order
    // so the primary model always gets the short alias (i == 0).
    let mut model_alias_map: HashMap<String, (String, &ResolvedModel)> = HashMap::new();
    for (i, name) in needed_models.iter().enumerate() {
        if let Some(model) = resolved_models.get(name) {
            let alias = if i == 0 {
                model.alias.chars().next().unwrap_or('t').to_string()
            } else {
                format!("{}{}", model.alias.chars().next().unwrap_or('t'), i)
            };
            model_alias_map.insert(name.clone(), (alias, model));
        }
    }

    let all_time_spines = load_all_time_spines_from_store(store);

    // Dimension-only queries (no metrics) — dispatch before metric validation.
    if spec.metrics.is_empty() {
        return compile_dimension_only_query(
            store,
            spec,
            &model_alias_map,
            &join_edges,
            &all_time_spines,
            dialect,
        );
    }

    // Validate the query spec against the resolved models.
    validate_spec(spec, &all_metrics, &model_alias_map)?;

    // Check if we can compile everything into a single query or need CTEs.
    //
    // The single-query path is only semantically correct when all metrics share
    // the same semantic model (base table).  Mixing models in one FROM/JOIN
    // introduces fanout and leaks one metric's filters into the other.
    // When models differ, each metric must compile to its own CTE and the
    // results are stitched together with FULL OUTER JOIN.
    let all_simple = top_level
        .iter()
        .all(|m| m.metric_type == MetricType::Simple);
    let all_same_model = top_level
        .iter()
        .filter_map(|m| m.agg_params.as_ref().map(|ap| ap.semantic_model.as_str()))
        .collect::<HashSet<_>>()
        .len()
        <= 1;

    let any_spine_join = top_level.iter().any(|m| m.join_to_timespine);
    let all_same_agg_time = {
        let agg_time_dims: HashSet<_> = top_level
            .iter()
            .filter_map(|m| m.agg_params.as_ref()?.agg_time_dimension.as_deref())
            .collect();
        agg_time_dims.len() <= 1
    };
    if all_simple && all_same_model && all_same_agg_time && !top_level.is_empty() && !any_spine_join
    {
        compile_simple_metrics(
            spec,
            &top_level,
            &all_metrics,
            &model_alias_map,
            &join_edges,
            &all_time_spines,
            dialect,
        )
    } else {
        // Mixed or complex metrics: use CTE approach.
        compile_complex_metrics(
            store,
            spec,
            &all_metrics,
            &model_alias_map,
            &resolved_models,
            &join_edges,
            dialect,
            &mut ctes,
            &mut final_select_columns,
            &mut final_from,
            &mut final_joins,
        )
    }
}

/// Recursively resolve a metric and all its dependencies.
fn resolve_metrics_recursive(
    store: &mut impl MetricStore,
    name: &str,
    resolved: &mut HashMap<String, ResolvedMetric>,
) -> Result<(), MetricFlowError> {
    if resolved.contains_key(name) {
        return Ok(());
    }

    let metric = resolve_metric(store, name)?;

    // Resolve input metrics for derived metrics.
    for input in &metric.input_metrics {
        resolve_metrics_recursive(store, &input.name, resolved)?;
    }

    // Resolve numerator/denominator for ratio metrics.
    if let Some(ref num) = metric.numerator {
        resolve_metrics_recursive(store, &num.name, resolved)?;
    }
    if let Some(ref den) = metric.denominator {
        resolve_metrics_recursive(store, &den.name, resolved)?;
    }

    // Resolve base/conversion metrics for conversion metrics.
    if let Some(ref cp) = metric.conversion_params {
        if !cp.base_metric.is_empty() {
            resolve_metrics_recursive(store, &cp.base_metric, resolved)?;
        }
        if !cp.conversion_metric.is_empty() {
            resolve_metrics_recursive(store, &cp.conversion_metric, resolved)?;
        }
    }

    // Resolve cumulative metric's input metric.
    if metric.metric_type == MetricType::Cumulative {
        // Cumulative metrics typically have one input metric in the metrics array.
        for input in &metric.input_metrics {
            resolve_metrics_recursive(store, &input.name, resolved)?;
        }
    }

    resolved.insert(name.to_string(), metric);
    Ok(())
}

/// Collect all semantic model names needed by the requested metrics.
fn collect_needed_models(
    all_metrics: &HashMap<String, ResolvedMetric>,
    requested: &[String],
) -> Result<Vec<String>, MetricFlowError> {
    let mut models: HashSet<String> = HashSet::new();

    fn collect_from_metric(
        metric: &ResolvedMetric,
        all: &HashMap<String, ResolvedMetric>,
        models: &mut HashSet<String>,
    ) {
        if let Some(ref ap) = metric.agg_params {
            if !ap.semantic_model.is_empty() {
                models.insert(ap.semantic_model.clone());
            }
        }
        for input in &metric.input_metrics {
            if let Some(m) = all.get(&input.name) {
                collect_from_metric(m, all, models);
            }
        }
        if let Some(ref num) = metric.numerator {
            if let Some(m) = all.get(&num.name) {
                collect_from_metric(m, all, models);
            }
        }
        if let Some(ref den) = metric.denominator {
            if let Some(m) = all.get(&den.name) {
                collect_from_metric(m, all, models);
            }
        }
        if let Some(ref cp) = metric.conversion_params {
            if let Some(m) = all.get(&cp.base_metric) {
                collect_from_metric(m, all, models);
            }
            if let Some(m) = all.get(&cp.conversion_metric) {
                collect_from_metric(m, all, models);
            }
        }
    }

    for name in requested {
        if let Some(metric) = all_metrics.get(name) {
            collect_from_metric(metric, all_metrics, &mut models);
        }
    }

    let mut result: Vec<String> = models.into_iter().collect();
    result.sort();
    Ok(result)
}

// ═══════════════════════════════════════════════════════════════════════════
// Dimension-only query compilation (no metrics)
// ═══════════════════════════════════════════════════════════════════════════

#[allow(clippy::cognitive_complexity)]
fn compile_dimension_only_query(
    store: &mut impl MetricStore,
    spec: &SemanticQuerySpec,
    model_aliases: &HashMap<String, (String, &ResolvedModel)>,
    join_edges: &[JoinEdge],
    all_time_spines: &[TimeSpine],
    dialect: Dialect,
) -> Result<String, MetricFlowError> {
    // Deterministic model ordering (same order as compile() builds needed_models).
    let needed_models: Vec<String> = {
        let mut v: Vec<String> = model_aliases.keys().cloned().collect();
        v.sort();
        v
    };

    let primary_model_name = needed_models.first().cloned().unwrap_or_default();

    // Check if metric_time is requested.
    let mut needs_time_spine = false;
    let mut finest_metric_time_gran = "day";
    for gb in &spec.group_by {
        if let GroupBySpec::TimeDimension {
            name, granularity, ..
        } = gb
        {
            if name == "metric_time" {
                needs_time_spine = true;
                if granularity_rank(granularity) < granularity_rank(finest_metric_time_gran) {
                    finest_metric_time_gran = granularity;
                }
            }
        }
    }

    // Check if we have metric filter references ({{ Metric(...) }}).
    if spec.where_filters.iter().any(|f| f.contains("Metric(")) {
        return compile_dimension_only_with_metric_filter(
            store,
            spec,
            model_aliases,
            join_edges,
            all_time_spines,
            dialect,
        );
    }

    // Determine if we need a subquery wrapper for WHERE filters that reference
    // dimensions not in the GROUP BY (need to SELECT them for WHERE, then project
    // only the GROUP BY columns out).
    let needs_subquery_wrapper = spec.where_filters.iter().any(|f| {
        let mut refs_entity = None;
        if let Some(start) = f.find("Dimension(") {
            // Skip if this is actually "TimeDimension("
            if start == 0
                || f.as_bytes()
                    .get(start.wrapping_sub(4)..start)
                    .is_none_or(|b| b != b"Time")
            {
                if let Some(end) = f[start..].find(')') {
                    let inner = &f[start + 10..start + end];
                    let dim_ref = inner.trim().trim_matches('\'').trim_matches('"');
                    if let Some((entity_part, _)) = dim_ref.rsplit_once("__") {
                        refs_entity = Some(entity_part.to_string());
                    }
                }
            }
        }
        if let Some(ref filter_entity) = refs_entity {
            !spec.group_by.iter().any(|gb| {
                matches!(gb,
                GroupBySpec::Dimension { entity: Some(entity), .. } if entity == filter_entity)
            })
        } else {
            false
        }
    });

    // Build SELECT columns, GROUP BY columns.
    let mut select_cols: Vec<String> = Vec::new();
    let mut group_by_cols: Vec<String> = Vec::new();
    let mut output_col_names: Vec<String> = Vec::new();
    let mut custom_gran_joins: Vec<(String, String, String)> = Vec::new();
    let mut cg_alias_counter = 0u32;

    let spine_alias = if needs_time_spine { "ts" } else { "" };

    for gb in &spec.group_by {
        match gb {
            GroupBySpec::TimeDimension {
                name,
                granularity,
                date_part,
            } => {
                if name == "metric_time" {
                    if let Some(spine) =
                        pick_time_spine_for_granularity(all_time_spines, finest_metric_time_gran)
                    {
                        let raw_col = format!("{spine_alias}.{}", spine.primary_column);
                        if let Some(dp) = date_part {
                            let expr = render_extract(
                                dp,
                                &format!("CAST({raw_col} AS TIMESTAMP)"),
                                dialect,
                            );
                            let alias = format!("metric_time__extract_{dp}");
                            select_cols.push(format!("{expr} AS {alias}"));
                            group_by_cols.push(expr);
                            output_col_names.push(alias);
                        } else {
                            let expr = render_date_trunc(
                                granularity,
                                &format!("CAST({raw_col} AS TIMESTAMP)"),
                                dialect,
                            );
                            let alias = format!("metric_time__{granularity}");
                            select_cols.push(format!("{expr} AS {alias}"));
                            group_by_cols.push(expr);
                            output_col_names.push(alias);
                        }
                    }
                } else if !is_standard_granularity(granularity) {
                    if let Some((spine, custom_col)) =
                        find_custom_granularity_spine(all_time_spines, granularity)
                    {
                        let cg_alias = if cg_alias_counter == 0 {
                            "ts_cg".to_string()
                        } else {
                            format!("ts_cg{cg_alias_counter}")
                        };
                        cg_alias_counter += 1;
                        let time_expr = resolve_time_dimension_ref(
                            name,
                            "day",
                            model_aliases,
                            dialect,
                            &primary_model_name,
                        );
                        let spine_rel = spine.relation_name.clone();
                        let on_cond = format!("{time_expr} = {cg_alias}.{}", spine.primary_column);
                        custom_gran_joins.push((cg_alias.clone(), spine_rel, on_cond));
                        let col_expr = format!("{cg_alias}.{custom_col}");
                        let alias = format!("{name}__{granularity}");
                        select_cols.push(format!("{col_expr} AS {alias}"));
                        group_by_cols.push(col_expr);
                        output_col_names.push(alias);
                    }
                } else {
                    let resolved = resolve_time_dimension_ref(
                        name,
                        granularity,
                        model_aliases,
                        dialect,
                        &primary_model_name,
                    );
                    let alias = format!("{name}__{granularity}");
                    select_cols.push(format!("{resolved} AS {alias}"));
                    group_by_cols.push(resolved);
                    output_col_names.push(alias);
                }
            }
            GroupBySpec::Dimension { entity, name } => {
                let full_ref = match entity {
                    Some(e) => format!("{e}__{name}"),
                    None => name.clone(),
                };
                let resolved =
                    resolve_dimension_ref(&full_ref, model_aliases, dialect, &primary_model_name);
                select_cols.push(format!("{resolved} AS {full_ref}"));
                group_by_cols.push(resolved);
                output_col_names.push(full_ref);
            }
            GroupBySpec::Entity { name } => {
                let resolved = resolve_entity_ref(name, model_aliases, &primary_model_name);
                select_cols.push(format!("{resolved} AS {name}"));
                group_by_cols.push(resolved);
                output_col_names.push(name.clone());
            }
        }
    }

    // Build FROM clause.
    let from_clause: String;
    let mut join_clauses: Vec<String> = Vec::new();

    if needs_time_spine && needed_models.is_empty() {
        let spine = pick_time_spine_for_granularity(all_time_spines, finest_metric_time_gran)
            .ok_or_else(|| MetricFlowError::Other("no time spine found".into()))?;
        from_clause = format!("{} {spine_alias}", spine.relation_name);
    } else if needed_models.is_empty() {
        return Err(MetricFlowError::Other(
            "dimension-only query has no models to select from".into(),
        ));
    } else {
        let (primary_alias, primary_model) = model_aliases
            .get(&primary_model_name)
            .ok_or_else(|| MetricFlowError::Other("primary model not resolved".into()))?;
        from_clause = format!("{} {primary_alias}", primary_model.relation_name);

        // Join additional models via join graph.
        for target_model in &needed_models {
            if *target_model == primary_model_name {
                continue;
            }
            if let Some(path) = find_join_path(join_edges, &primary_model_name, target_model) {
                let mut prev = primary_model_name.clone();
                for edge in &path {
                    let (ta, tm) = model_aliases.get(&edge.to_model).ok_or_else(|| {
                        MetricFlowError::Other(format!("model not resolved: {}", edge.to_model))
                    })?;
                    let (pa, _) = model_aliases.get(&prev).ok_or_else(|| {
                        MetricFlowError::Other(format!("model not resolved: {prev}"))
                    })?;
                    let cond = format!("{pa}.{} = {ta}.{}", edge.from_expr, edge.to_expr);
                    if !join_clauses
                        .iter()
                        .any(|j| j.contains(&format!("{} {ta}", tm.relation_name)))
                    {
                        join_clauses.push(format!(
                            "FULL OUTER JOIN\n  {} {ta}\nON\n  {cond}",
                            tm.relation_name
                        ));
                    }
                    prev = edge.to_model.clone();
                }
            }
        }

        if needs_time_spine {
            let spine = pick_time_spine_for_granularity(all_time_spines, finest_metric_time_gran)
                .ok_or_else(|| MetricFlowError::Other("no time spine found".into()))?;
            join_clauses.insert(
                0,
                format!("CROSS JOIN\n  {} {spine_alias}", spine.relation_name),
            );
        }
    }

    // WHERE clause.
    let mut where_parts: Vec<String> = Vec::new();
    let mut dummy_sql = String::new();
    for filter in &spec.where_filters {
        where_parts.push(resolve_where_filter_custom_gran(
            filter,
            model_aliases,
            dialect,
            &primary_model_name,
            all_time_spines,
            None,
            &mut custom_gran_joins,
            &mut cg_alias_counter,
            &mut dummy_sql,
        ));
    }

    // Time constraint on metric_time.
    if let Some((ref start, ref end)) = spec.time_constraint {
        for gb in &spec.group_by {
            if let GroupBySpec::TimeDimension {
                name, granularity, ..
            } = gb
            {
                if name == "metric_time" {
                    if let Some(spine) =
                        pick_time_spine_for_granularity(all_time_spines, finest_metric_time_gran)
                    {
                        let raw = format!("{spine_alias}.{}", spine.primary_column);
                        let trunc = render_date_trunc(
                            granularity,
                            &format!("CAST({raw} AS TIMESTAMP)"),
                            dialect,
                        );
                        let cast_trunc = render_type_cast(&trunc, "TIMESTAMP", dialect);
                        let start_ts =
                            render_type_cast(&format!("'{start}'"), "TIMESTAMP", dialect);
                        let has_time = end.contains(' ') || end.contains('T');
                        let subdaily = matches!(granularity.as_str(), "hour" | "minute" | "second");
                        if has_time && subdaily {
                            let end_bound = format!(
                                "DATE_TRUNC('{granularity}', CAST('{end}' AS TIMESTAMP)) + INTERVAL '1 {granularity}'"
                            );
                            where_parts.push(format!(
                                "{cast_trunc} >= {start_ts} AND {cast_trunc} < {end_bound}"
                            ));
                        } else if has_time {
                            let end_ts =
                                render_type_cast(&format!("'{end}'"), "TIMESTAMP", dialect);
                            where_parts.push(format!(
                                "{cast_trunc} >= {start_ts} AND {cast_trunc} <= {end_ts}"
                            ));
                        } else {
                            let end_bound =
                                format!("CAST('{end}' AS TIMESTAMP) + INTERVAL '1 day'");
                            where_parts.push(format!(
                                "{cast_trunc} >= {start_ts} AND {cast_trunc} < {end_bound}"
                            ));
                        }
                    }
                    break;
                }
            }
        }
    }

    // Assemble.
    if needs_subquery_wrapper {
        // For subquery wrapping, add WHERE filter dimensions to the inner SELECT.
        let mut inner_select_cols = select_cols.clone();
        let mut all_output_names = output_col_names.clone();
        for filter in &spec.where_filters {
            let mut cursor = 0usize;
            while let Some(pos) = filter[cursor..].find("Dimension(") {
                let abs_pos = cursor + pos;
                if abs_pos > 0 && filter[..abs_pos].ends_with("Time") {
                    cursor = abs_pos + 10;
                    continue;
                }
                if let Some(paren_end) = filter[abs_pos..].find(')') {
                    let inner = &filter[abs_pos + 10..abs_pos + paren_end];
                    let dim_ref = inner.trim().trim_matches('\'').trim_matches('"');
                    let col_alias = dim_ref.to_string();
                    if !all_output_names.contains(&col_alias) {
                        let resolved = resolve_dimension_ref(
                            dim_ref,
                            model_aliases,
                            dialect,
                            &primary_model_name,
                        );
                        inner_select_cols.push(format!("{resolved} AS {col_alias}"));
                        all_output_names.push(col_alias);
                    }
                    cursor = abs_pos + paren_end + 1;
                } else {
                    break;
                }
            }
        }

        let inner_select = inner_select_cols.join("\n  , ");
        let mut inner_sql = format!("SELECT\n  {inner_select}\nFROM {from_clause}");
        for join in &join_clauses {
            inner_sql.push_str(&format!("\n{join}"));
        }
        for (cg_alias, cg_relation, cg_on) in &custom_gran_joins {
            inner_sql.push_str(&format!(
                "\nLEFT OUTER JOIN {cg_relation} {cg_alias}\nON {cg_on}"
            ));
        }
        let outer_cols: Vec<&str> = output_col_names
            .iter()
            .filter(|n| {
                spec.group_by.iter().any(|gb| match gb {
                    GroupBySpec::Dimension { entity, name } => {
                        **n == match entity {
                            Some(e) => format!("{e}__{name}"),
                            None => name.clone(),
                        }
                    }
                    GroupBySpec::TimeDimension {
                        name, granularity, ..
                    } => **n == format!("{name}__{granularity}"),
                    GroupBySpec::Entity { name } => *n == name,
                })
            })
            .map(|s| s.as_str())
            .collect();
        let outer_select = outer_cols.join("\n  , ");
        let mut sql = format!(
            "SELECT\n  {outer_select}\nFROM (\n  {}\n) outer_subq",
            inner_sql.replace('\n', "\n  ")
        );
        // Resolve WHERE against output column aliases (not model aliases).
        if !spec.where_filters.is_empty() {
            let mut resolved_filters: Vec<String> = Vec::new();
            for filter in &spec.where_filters {
                let mut resolved = filter.clone();
                while let Some(start) = resolved.find("{{ Dimension(") {
                    if let Some(end) = resolved[start..].find("}}").map(|i| start + i + 2) {
                        let snippet = resolved[start + 2..end - 2].trim().to_string();
                        let dim_ref = snippet
                            .strip_prefix("Dimension(")
                            .and_then(|s| s.strip_suffix(')'))
                            .unwrap_or(&snippet)
                            .trim()
                            .trim_matches('\'')
                            .trim_matches('"')
                            .to_string();
                        resolved.replace_range(start..end, &dim_ref);
                    } else {
                        break;
                    }
                }
                resolved_filters.push(resolved);
            }
            sql.push_str(&format!("\nWHERE {}", resolved_filters.join(" AND ")));
        }
        if spec.apply_group_by {
            sql.push_str(&format!("\nGROUP BY\n  {}", outer_cols.join("\n  , ")));
        }
        if !spec.order_by.is_empty() {
            let parts: Vec<String> = spec
                .order_by
                .iter()
                .map(|o| {
                    let col = resolve_order_by_col(&o.name, &spec.group_by);
                    if o.descending {
                        format!("{col} DESC")
                    } else {
                        col
                    }
                })
                .collect();
            sql.push_str(&format!("\nORDER BY\n  {}", parts.join("\n  , ")));
        }
        if let Some(limit) = spec.limit {
            sql.push_str(&format!("\nLIMIT {limit}"));
        }
        Ok(sql)
    } else {
        let select = select_cols.join("\n  , ");
        let mut sql = format!("SELECT\n  {select}\nFROM {from_clause}");
        for join in &join_clauses {
            sql.push_str(&format!("\n{join}"));
        }
        for (cg_alias, cg_relation, cg_on) in &custom_gran_joins {
            sql.push_str(&format!(
                "\nLEFT OUTER JOIN {cg_relation} {cg_alias}\nON {cg_on}"
            ));
        }
        if !where_parts.is_empty() {
            sql.push_str(&format!("\nWHERE {}", where_parts.join(" AND ")));
        }
        if spec.apply_group_by {
            sql.push_str(&format!("\nGROUP BY\n  {}", group_by_cols.join("\n  , ")));
        }
        if !spec.order_by.is_empty() {
            let parts: Vec<String> = spec
                .order_by
                .iter()
                .map(|o| {
                    let col = resolve_order_by_col(&o.name, &spec.group_by);
                    if o.descending {
                        format!("{col} DESC")
                    } else {
                        col
                    }
                })
                .collect();
            sql.push_str(&format!("\nORDER BY\n  {}", parts.join("\n  , ")));
        }
        if let Some(limit) = spec.limit {
            sql.push_str(&format!("\nLIMIT {limit}"));
        }
        Ok(sql)
    }
}

fn compile_dimension_only_with_metric_filter(
    store: &mut impl MetricStore,
    spec: &SemanticQuerySpec,
    model_aliases: &HashMap<String, (String, &ResolvedModel)>,
    _join_edges: &[JoinEdge],
    _all_time_spines: &[TimeSpine],
    dialect: Dialect,
) -> Result<String, MetricFlowError> {
    let entity_for_join = spec
        .group_by
        .iter()
        .find_map(|gb| {
            if let GroupBySpec::Entity { name } = gb {
                Some(name.clone())
            } else {
                None
            }
        })
        .unwrap_or_default();

    let metric_filter_refs = extract_metric_filter_refs(&spec.where_filters);

    let mut all_metrics: HashMap<String, ResolvedMetric> = HashMap::new();
    for mfr in &metric_filter_refs {
        resolve_metrics_recursive(store, &mfr.metric_name, &mut all_metrics)?;
    }

    // Build metric subquery CTEs.
    let mut cte_sqls: Vec<(String, String)> = Vec::new();
    let mut metric_model_names: Vec<String> = Vec::new();
    for mfr in &metric_filter_refs {
        if let Some(metric) = all_metrics.get(&mfr.metric_name) {
            if let Some(ref ap) = metric.agg_params {
                let entity_name = mfr.entities.first().map(|s| s.as_str()).unwrap_or("id");
                let model = resolve_model(store, &ap.semantic_model)?;
                metric_model_names.push(ap.semantic_model.clone());
                let agg_expr = render_agg_with_params(
                    &ap.agg,
                    &ap.expr,
                    dialect,
                    ap.percentile,
                    ap.use_discrete_percentile,
                );
                let entity_expr = model
                    .entities
                    .iter()
                    .find(|e| e.name == entity_name)
                    .map(|e| e.expr.clone())
                    .unwrap_or_else(|| entity_name.to_string());
                cte_sqls.push((
                    format!("mf__{}", mfr.metric_name),
                    format!("SELECT\n  {entity_expr} AS {entity_name}\n  , {agg_expr} AS {}\nFROM {}\nGROUP BY {entity_expr}",
                        mfr.metric_name, model.relation_name),
                ));
            }
        }
    }

    // Pick the base model: prefer a pure dimension/mapping table for the entity,
    // excluding models that back the metric subqueries.
    let exclude_models: Vec<&str> = metric_model_names.iter().map(|s| s.as_str()).collect();
    let primary_model_name =
        find_dimension_model_for_entity(store, &entity_for_join, &exclude_models)?
            .or_else(|| model_aliases.keys().next().cloned())
            .unwrap_or_default();

    let base_model = resolve_model(store, &primary_model_name)?;
    let base_alias = base_model.alias.chars().next().unwrap_or('t').to_string();
    let from_model = format!("{} {base_alias}", base_model.relation_name);
    let entity_col = base_model
        .entities
        .iter()
        .find(|e| e.name == entity_for_join)
        .map(|e| format!("{base_alias}.{}", e.expr))
        .unwrap_or_else(|| format!("{base_alias}.{entity_for_join}"));

    // Inner subquery: entity column + metric columns.
    let mut inner_select: Vec<String> = Vec::new();
    for gb in &spec.group_by {
        if let GroupBySpec::Entity { name } = gb {
            inner_select.push(format!("{entity_col} AS {name}"));
        }
    }
    let mut inner_joins: Vec<String> = Vec::new();
    for (cte_name, _) in &cte_sqls {
        let metric_name = cte_name.strip_prefix("mf__").unwrap_or(cte_name);
        inner_select.push(format!(
            "a.{metric_name} AS {entity_for_join}__{metric_name}"
        ));
        inner_joins.push(format!(
            "FULL OUTER JOIN (\n  SELECT * FROM {cte_name}\n) a\nON {entity_col} = a.{entity_for_join}"));
    }

    let mut sql = String::new();
    if !cte_sqls.is_empty() {
        sql.push_str("WITH ");
        for (i, (name, body)) in cte_sqls.iter().enumerate() {
            if i > 0 {
                sql.push_str(", ");
            }
            sql.push_str(&format!("{name} AS (\n  {}\n)", body.replace('\n', "\n  ")));
        }
        sql.push('\n');
    }

    let mut inner_sql = format!(
        "SELECT\n    {}\n  FROM {from_model}",
        inner_select.join("\n    , ")
    );
    for join in &inner_joins {
        inner_sql.push_str(&format!("\n  {join}"));
    }

    let outer_cols: Vec<String> = spec
        .group_by
        .iter()
        .map(|gb| match gb {
            GroupBySpec::Entity { name } => name.clone(),
            GroupBySpec::Dimension { entity, name } => match entity {
                Some(e) => format!("{e}__{name}"),
                None => name.clone(),
            },
            GroupBySpec::TimeDimension {
                name, granularity, ..
            } => format!("{name}__{granularity}"),
        })
        .collect();

    // Resolve WHERE — replace {{ Metric(...) }} with column refs.
    let mut where_clause = String::new();
    for filter in &spec.where_filters {
        let mut resolved = filter.clone();
        while let Some(start) = resolved.find("{{ Metric(") {
            if let Some(end) = resolved[start..].find("}}").map(|i| start + i + 2) {
                let inner = resolved[start + 2..end - 2].trim();
                let mfr_inner = inner
                    .strip_prefix("Metric(")
                    .and_then(|s| s.strip_suffix(')'))
                    .unwrap_or(inner);
                let metric_name = mfr_inner
                    .split(',')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_matches('\'')
                    .trim_matches('"');
                resolved.replace_range(start..end, &format!("{entity_for_join}__{metric_name}"));
            } else {
                break;
            }
        }
        where_clause = resolved;
    }

    sql.push_str(&format!(
        "SELECT\n  {}\nFROM (\n  {}\n) outer_subq",
        outer_cols.join("\n  , "),
        inner_sql.replace('\n', "\n  ")
    ));
    if !where_clause.is_empty() {
        sql.push_str(&format!("\nWHERE {where_clause}"));
    }
    sql.push_str(&format!("\nGROUP BY {}", outer_cols.join(", ")));
    if !spec.order_by.is_empty() {
        let parts: Vec<String> = spec
            .order_by
            .iter()
            .map(|o| {
                let col = resolve_order_by_col(&o.name, &spec.group_by);
                if o.descending {
                    format!("{col} DESC")
                } else {
                    col
                }
            })
            .collect();
        sql.push_str(&format!("\nORDER BY {}", parts.join(", ")));
    }
    if let Some(limit) = spec.limit {
        sql.push_str(&format!("\nLIMIT {limit}"));
    }
    Ok(sql)
}

// ═══════════════════════════════════════════════════════════════════════════
// Simple metric compilation (single-query path)
// ═══════════════════════════════════════════════════════════════════════════

#[allow(clippy::cognitive_complexity)]
fn compile_simple_metrics(
    spec: &SemanticQuerySpec,
    metrics: &[&ResolvedMetric],
    all_metrics: &HashMap<String, ResolvedMetric>,
    model_aliases: &HashMap<String, (String, &ResolvedModel)>,
    join_edges: &[JoinEdge],
    all_time_spines: &[TimeSpine],
    dialect: Dialect,
) -> Result<String, MetricFlowError> {
    let mut sql = String::new();

    // Determine the primary model (first metric's model).
    let primary_model_name = metrics
        .first()
        .and_then(|m| m.agg_params.as_ref())
        .map(|ap| ap.semantic_model.clone())
        .ok_or_else(|| {
            MetricFlowError::Other("no semantic model found for simple metrics".into())
        })?;

    let (primary_alias, primary_model) =
        model_aliases.get(&primary_model_name).ok_or_else(|| {
            MetricFlowError::Other(format!("semantic model not resolved: {primary_model_name}"))
        })?;

    // Collect target models that need to be reachable from the primary model.
    // We first gather the *target* model names, then resolve full join paths
    // (which may include intermediate hops).
    let mut target_models: Vec<String> = Vec::new();
    let mut target_set: HashSet<String> = HashSet::new();

    // 1. Metrics from other models.
    for metric in metrics {
        if let Some(ref ap) = metric.agg_params {
            if !ap.semantic_model.is_empty()
                && ap.semantic_model != primary_model_name
                && !target_set.contains(&ap.semantic_model)
            {
                target_models.push(ap.semantic_model.clone());
                target_set.insert(ap.semantic_model.clone());
            }
        }
    }

    // 2. Dimensions from other models (via entity references like Dimension('listing__country')).
    for gb in &spec.group_by {
        if let GroupBySpec::Dimension {
            entity: Some(entity_name),
            name: dim_name,
        } = gb
        {
            // For multi-hop entity chains (e.g. "account_id__customer_id"),
            // add all models reachable via any entity in the chain.
            let entity_segments: Vec<&str> = entity_name.split("__").collect();
            // Prefer the model that has both the entity AND the dimension over
            // models that merely share the entity name.
            let mut best_match: Option<&String> = None;
            let mut entity_only_match: Option<&String> = None;
            for (model_name, (_alias, model)) in model_aliases.iter() {
                if model_name == &primary_model_name || target_set.contains(model_name) {
                    continue;
                }
                let has_entity = entity_segments
                    .iter()
                    .any(|seg| model.entities.iter().any(|e| e.name == *seg));
                if !has_entity {
                    continue;
                }
                let has_dim = model.dimensions.iter().any(|d| d.name == *dim_name);
                if has_dim && best_match.is_none() {
                    best_match = Some(model_name);
                } else if entity_only_match.is_none() {
                    entity_only_match = Some(model_name);
                }
            }
            let selected = best_match.or(entity_only_match);
            if let Some(model_name) = selected {
                if !target_set.contains(model_name) {
                    target_models.push(model_name.clone());
                    target_set.insert(model_name.clone());
                }
            }
        }
    }

    // 2b. TimeDimension with entity prefix (e.g. TimeDimension('user__last_login_ts', 'minute')).
    // For multi-hop chains like 'listing__user__ds', find the model where the target
    // entity is primary and that has the dimension. For intermediate entities, find
    // the model where that entity is primary (the join path resolution will add
    // intermediates automatically).
    for gb in &spec.group_by {
        if let GroupBySpec::TimeDimension { name, .. } = gb {
            if let Some((entity_chain, dim_name)) = name.rsplit_once("__") {
                let target_entity = entity_chain
                    .rsplit_once("__")
                    .map(|(_, last)| last)
                    .unwrap_or(entity_chain);
                // Find the model where the target entity is primary AND has the dimension.
                let mut found = false;
                for (model_name, (_alias, model)) in model_aliases.iter() {
                    if model_name == &primary_model_name || target_set.contains(model_name) {
                        continue;
                    }
                    let is_primary = model.entities.iter().any(|e| {
                        e.name == target_entity
                            && matches!(e.entity_type.as_str(), "primary" | "unique" | "natural")
                    }) || model.primary_entity.as_deref() == Some(target_entity);
                    if !is_primary {
                        continue;
                    }
                    let has_dim = model.dimensions.iter().any(|d| d.name == dim_name);
                    if has_dim {
                        target_models.push(model_name.clone());
                        target_set.insert(model_name.clone());
                        found = true;
                        break;
                    }
                }
                // Fallback: any model with the target entity as primary.
                if !found {
                    for (model_name, (_alias, model)) in model_aliases.iter() {
                        if model_name == &primary_model_name || target_set.contains(model_name) {
                            continue;
                        }
                        let is_primary = model.entities.iter().any(|e| {
                            e.name == target_entity
                                && matches!(
                                    e.entity_type.as_str(),
                                    "primary" | "unique" | "natural"
                                )
                        }) || model.primary_entity.as_deref()
                            == Some(target_entity);
                        if is_primary {
                            target_models.push(model_name.clone());
                            target_set.insert(model_name.clone());
                            break;
                        }
                    }
                }
            }
        }
    }

    // 3. Entity group-by specs (e.g. Entity('lux_listing')) — find models with that entity.
    for gb in &spec.group_by {
        if let GroupBySpec::Entity { name: entity_name } = gb {
            // Check if the primary model already has this entity.
            let primary_has = primary_model
                .entities
                .iter()
                .any(|e| e.name == *entity_name);
            if !primary_has {
                for (model_name, (_alias, model)) in model_aliases {
                    if model_name == &primary_model_name || target_set.contains(model_name) {
                        continue;
                    }
                    let has_entity = model.entities.iter().any(|e| e.name == *entity_name);
                    if has_entity {
                        target_models.push(model_name.clone());
                        target_set.insert(model_name.clone());
                    }
                }
            }
        }
    }

    // 4. Where filters with entity-prefixed dimension references.
    for filter in spec
        .where_filters
        .iter()
        .chain(metrics.iter().flat_map(|m| m.metric_filters.iter()))
    {
        let mut cursor = 0usize;
        while let Some(dim_start) = filter[cursor..].find("Dimension(") {
            let abs_start = cursor + dim_start + 10;
            if let Some(paren_end) = filter[abs_start..].find(')') {
                let dim_ref = filter[abs_start..abs_start + paren_end]
                    .trim()
                    .trim_matches('\'')
                    .trim_matches('"');
                if let Some((entity_name, _)) = dim_ref.split_once("__") {
                    for (model_name, (_alias, model)) in model_aliases.iter() {
                        if model_name == &primary_model_name || target_set.contains(model_name) {
                            continue;
                        }
                        let has_entity = model.entities.iter().any(|e| e.name == entity_name);
                        if has_entity {
                            target_models.push(model_name.clone());
                            target_set.insert(model_name.clone());
                        }
                    }
                }
                cursor = abs_start + paren_end + 1;
            } else {
                break;
            }
        }
    }

    // Resolve full join paths for each target model, collecting all intermediate
    // models along the way.  Each entry is (edge, target_model_alias, target_model).
    // We track which models are already in the join set to avoid duplicates.
    // Models that can't be reached via join edges get a CROSS JOIN fallback.
    let mut join_sequence: Vec<JoinEdge> = Vec::new();
    let mut cross_join_models: Vec<String> = Vec::new();
    let mut joined_set: HashSet<String> = HashSet::new();
    joined_set.insert(primary_model_name.clone());

    for target in &target_models {
        if joined_set.contains(target) {
            continue;
        }
        if let Some(path) = find_join_path(join_edges, &primary_model_name, target) {
            for edge in &path {
                if !joined_set.contains(&edge.to_model) {
                    joined_set.insert(edge.to_model.clone());
                    join_sequence.push(edge.clone());
                }
            }
        } else {
            // No join path found — mark for CROSS JOIN.
            joined_set.insert(target.clone());
            cross_join_models.push(target.clone());
        }
    }

    // Plan multi-hop subquery joins.
    let all_metric_filters: Vec<String> = metrics
        .iter()
        .flat_map(|m| m.metric_filters.iter().cloned())
        .collect();
    let mh_subqueries = plan_multi_hop_joins(
        spec,
        &primary_model_name,
        model_aliases,
        join_edges,
        dialect,
        &all_metric_filters,
    );

    // Build SELECT columns.
    let _ = writeln!(sql, "SELECT");

    let mut select_parts: Vec<String> = Vec::new();
    let out_cols = group_by_output_cols(&spec.group_by);

    // Determine the agg_time_dimension for metric_time resolution.
    // When all metrics share the same agg_time_dimension, use it to resolve metric_time.
    let shared_agg_time_dim: Option<&str> = if metrics.len() == 1 {
        metrics[0]
            .agg_params
            .as_ref()
            .and_then(|ap| ap.agg_time_dimension.as_deref())
    } else {
        let first = metrics[0]
            .agg_params
            .as_ref()
            .and_then(|ap| ap.agg_time_dimension.as_deref());
        if metrics.iter().all(|m| {
            m.agg_params
                .as_ref()
                .and_then(|ap| ap.agg_time_dimension.as_deref())
                == first
        }) {
            first
        } else {
            None
        }
    };

    // Collect custom granularity JOINs needed.
    let mut custom_gran_joins: Vec<(String, String, String)> = Vec::new(); // (alias, relation, on_cond)
    let mut cg_alias_counter = 0u32;

    // Group-by columns.
    for (gb, out_col) in spec.group_by.iter().zip(out_cols.iter()) {
        match gb {
            GroupBySpec::TimeDimension {
                name,
                granularity,
                date_part,
            } => {
                if !is_standard_granularity(granularity) {
                    if let Some((spine, custom_col)) =
                        find_custom_granularity_spine(all_time_spines, granularity)
                    {
                        let cg_alias = if cg_alias_counter == 0 {
                            "ts_cg".to_string()
                        } else {
                            format!("ts_cg{cg_alias_counter}")
                        };
                        cg_alias_counter += 1;
                        let time_expr = resolve_time_dimension_ref_with_agg(
                            name,
                            "day",
                            model_aliases,
                            dialect,
                            &primary_model_name,
                            shared_agg_time_dim,
                        );
                        let spine_rel = match dialect {
                            Dialect::Databricks => spine.relation_name.replace('"', "`"),
                            _ => spine.relation_name.clone(),
                        };
                        let on_cond = format!("{time_expr} = {cg_alias}.{}", spine.primary_column);
                        custom_gran_joins.push((cg_alias.clone(), spine_rel, on_cond));
                        let col_expr = format!("{cg_alias}.{custom_col}");
                        if let Some(part) = date_part {
                            let extract_expr = render_extract(part, &col_expr, dialect);
                            select_parts.push(format!("  {extract_expr} AS {out_col}"));
                        } else {
                            select_parts.push(format!("  {col_expr} AS {out_col}"));
                        }
                    } else {
                        return Err(MetricFlowError::Other(format!(
                            "unknown custom granularity: {granularity}"
                        )));
                    }
                } else {
                    let col_expr = resolve_time_dimension_ref_with_agg(
                        name,
                        granularity,
                        model_aliases,
                        dialect,
                        &primary_model_name,
                        shared_agg_time_dim,
                    );
                    if let Some(part) = date_part {
                        let extract_expr = render_extract(part, &col_expr, dialect);
                        select_parts.push(format!("  {extract_expr} AS {out_col}"));
                    } else {
                        select_parts.push(format!("  {col_expr} AS {out_col}"));
                    }
                }
            }
            GroupBySpec::Dimension { entity, name } => {
                let dim_ref = match entity {
                    Some(e) => format!("{e}__{name}"),
                    None => name.clone(),
                };
                let col_expr = resolve_dimension_ref_with_mh(
                    &dim_ref,
                    model_aliases,
                    dialect,
                    &primary_model_name,
                    &mh_subqueries,
                );
                select_parts.push(format!("  {col_expr} AS {out_col}"));
            }
            GroupBySpec::Entity { name } => {
                let col_expr = resolve_entity_ref(name, model_aliases, &primary_model_name);
                select_parts.push(format!("  {col_expr} AS {out_col}"));
            }
        }
    }

    // Metric columns.
    // When a metric has metric_filters AND other metrics in this query don't have the
    // same filter, wrap the measure in CASE WHEN so the filter applies only to this metric.
    let any_metric_has_filter = metrics.iter().any(|m| !m.metric_filters.is_empty());
    let all_share_same_filters = metrics
        .windows(2)
        .all(|w| w[0].metric_filters == w[1].metric_filters);
    let use_case_when = any_metric_has_filter && !all_share_same_filters;

    for metric in metrics {
        if let Some(ref ap) = metric.agg_params {
            let model_alias = model_aliases
                .get(&ap.semantic_model)
                .map(|(a, _)| a.as_str())
                .unwrap_or("t");
            let col_expr =
                if ap.semantic_model != primary_model_name && is_complex_measure_expr(&ap.expr) {
                    format!("{model_alias}.__mf_{}_expr", metric.name)
                } else {
                    qualify_measure_expr(model_alias, &ap.expr)
                };
            let filtered_expr = if use_case_when && !metric.metric_filters.is_empty() {
                let conds: Vec<String> = metric
                    .metric_filters
                    .iter()
                    .map(|f| resolve_where_filter(f, model_aliases, dialect, &primary_model_name))
                    .collect();
                format!("CASE WHEN {} THEN {col_expr} END", conds.join(" AND "))
            } else {
                col_expr
            };
            let agg_expr = render_agg_with_params(
                &ap.agg,
                &filtered_expr,
                dialect,
                ap.percentile,
                ap.use_discrete_percentile,
            );
            select_parts.push(format!("  {agg_expr} AS {}", metric.name));
        }
    }

    sql.push_str(&select_parts.join(",\n"));
    let _ = writeln!(sql);

    // FROM clause.
    let from_relation = render_full_relation(primary_model, dialect);
    let _ = writeln!(sql, "FROM {from_relation} AS {primary_alias}");

    // For secondary models whose measure expression is complex (e.g. a CASE expression),
    // we wrap the JOIN target in a derived table that pre-computes the expression as a
    // named column.  This makes all column references unambiguous by construction —
    // inside the subquery only one table is in scope, so no alias qualification is needed.
    let mut complex_exprs_by_model: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for metric in metrics.iter() {
        if let Some(ref ap) = metric.agg_params {
            if ap.semantic_model != primary_model_name && is_complex_measure_expr(&ap.expr) {
                complex_exprs_by_model
                    .entry(ap.semantic_model.clone())
                    .or_default()
                    .push((metric.name.clone(), ap.expr.clone()));
            }
        }
    }

    // Emit multi-hop subquery joins (independent bridge-through-leaf paths).
    // Collect models that are covered by subqueries so we skip them in flat joins.
    let mut mh_covered_models: HashSet<String> = HashSet::new();
    for mh in &mh_subqueries {
        let _ = writeln!(
            sql,
            "LEFT JOIN ({}) AS {} ON {} = {}.{}",
            mh.subquery_sql, mh.alias, mh.fact_join_expr, mh.alias, mh.subquery_join_col,
        );
        // Parse model names from the subquery to mark them as covered.
        for (model_name, (_, model)) in model_aliases {
            if model_name == &primary_model_name {
                continue;
            }
            let rel = render_full_relation(model, dialect);
            if mh.subquery_sql.contains(&rel) {
                mh_covered_models.insert(model_name.clone());
            }
        }
    }

    // JOIN clauses — emit one LEFT JOIN per edge in the resolved join sequence.
    for edge in &join_sequence {
        // Skip models that are already covered by multi-hop subqueries.
        if mh_covered_models.contains(&edge.to_model) {
            continue;
        }
        let (join_alias, join_model) = match model_aliases.get(&edge.to_model) {
            Some((a, m)) => (a.as_str(), *m),
            None => continue,
        };
        let left_alias = if edge.from_model == primary_model_name {
            primary_alias.as_str()
        } else {
            model_aliases
                .get(&edge.from_model)
                .map(|(a, _)| a.as_str())
                .unwrap_or(primary_alias.as_str())
        };
        let join_relation = render_full_relation(join_model, dialect);
        // Build join condition: entity key + any shared partition dimensions.
        let mut join_conds = vec![format!(
            "{left_alias}.{} = {join_alias}.{}",
            edge.from_expr, edge.to_expr
        )];
        // Add partition dimension matching: check from-model AND primary model.
        let from_model_opt = model_aliases.get(&edge.from_model).map(|(_, m)| *m);
        if let Some(from_model_ref) = from_model_opt {
            for from_dim in &from_model_ref.dimensions {
                if from_dim.is_partition {
                    if let Some(to_dim) = join_model
                        .dimensions
                        .iter()
                        .find(|d| d.is_partition && d.name == from_dim.name)
                    {
                        join_conds.push(format!(
                            "{left_alias}.{} = {join_alias}.{}",
                            from_dim.expr, to_dim.expr
                        ));
                    }
                }
            }
        }
        // For multi-hop joins, also check if the primary model shares a partition
        // dimension with the target model (even if intermediates don't have it).
        if edge.from_model != primary_model_name {
            for prim_dim in &primary_model.dimensions {
                if prim_dim.is_partition {
                    if let Some(to_dim) = join_model
                        .dimensions
                        .iter()
                        .find(|d| d.is_partition && d.name == prim_dim.name)
                    {
                        let cond = format!(
                            "{primary_alias}.{} = {join_alias}.{}",
                            prim_dim.expr, to_dim.expr
                        );
                        if !join_conds.contains(&cond) {
                            join_conds.push(cond);
                        }
                    }
                }
            }
        }
        // SCD temporal range condition: when the joined model is a slowly changing
        // dimension, add fact.time >= scd.valid_from AND (fact.time < scd.valid_to OR
        // scd.valid_to IS NULL) using the primary model's agg_time_dimension.
        if let (Some(vf), Some(vt)) = (
            join_model.scd_valid_from.as_deref(),
            join_model.scd_valid_to.as_deref(),
        ) {
            if let Some(fte) = shared_agg_time_dim.and_then(|atd| {
                primary_model
                    .dimensions
                    .iter()
                    .find(|d| d.name == atd)
                    .map(|d| format!("{primary_alias}.{}", d.expr))
            }) {
                join_conds.push(format!("{fte} >= {join_alias}.{vf}"));
                join_conds.push(format!(
                    "({fte} < {join_alias}.{vt} OR {join_alias}.{vt} IS NULL)"
                ));
            }
        }
        let join_on = join_conds.join(" AND ");
        if let Some(complex_exprs) = complex_exprs_by_model.get(&edge.to_model) {
            let derived_cols: Vec<String> = complex_exprs
                .iter()
                .map(|(name, expr)| format!("  {expr} AS __mf_{name}_expr"))
                .collect();
            let _ = writeln!(
                sql,
                "LEFT JOIN (\n  SELECT *,\n{}\n  FROM {join_relation}\n) AS {join_alias} ON {join_on}",
                derived_cols.join(",\n"),
            );
        } else {
            let _ = writeln!(
                sql,
                "LEFT JOIN {join_relation} AS {join_alias} ON {join_on}",
            );
        }
    }

    // CROSS JOIN for models that have no entity-based join path.
    for model_name in &cross_join_models {
        if let Some((join_alias, join_model)) = model_aliases.get(model_name) {
            let join_relation = render_full_relation(join_model, dialect);
            let _ = writeln!(sql, "CROSS JOIN {join_relation} AS {join_alias}");
        }
    }

    // Custom granularity JOINs are deferred until WHERE filters are resolved
    // (they may add more custom_gran_joins). Emitted below before the WHERE clause.

    // Semi-additive measure handling: INNER JOIN to subquery with MIN/MAX on
    // the non-additive time dimension.
    for metric in metrics {
        if let Some(ref ap) = metric.agg_params {
            if let Some(ref nad) = ap.non_additive_dimension {
                let nad_name = nad.get("name").and_then(|v| v.as_str()).unwrap_or("ds");
                let window_choice = nad
                    .get("window_choice")
                    .and_then(|v| v.as_str())
                    .unwrap_or("max");
                let window_groupings: Vec<&str> = nad
                    .get("window_groupings")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();

                let nad_dim = primary_model.dimensions.iter().find(|d| d.name == nad_name);
                let nad_expr = nad_dim.map(|d| d.expr.as_str()).unwrap_or(nad_name);
                let nad_gran = nad_dim
                    .and_then(|d| d.time_granularity.as_deref())
                    .unwrap_or("day");
                let agg_fn = if window_choice == "min" { "MIN" } else { "MAX" };

                let grouping_exprs: Vec<(String, String)> = window_groupings
                    .iter()
                    .filter_map(|&g| {
                        primary_model
                            .entities
                            .iter()
                            .find(|e| e.name == g)
                            .map(|e| (g.to_string(), e.expr.clone()))
                    })
                    .collect();

                let mut nad_select = Vec::new();
                let mut nad_join_conds = Vec::new();
                let mut group_count = 0usize;
                for (name, expr) in &grouping_exprs {
                    nad_select.push(format!("{expr} AS {name}"));
                    nad_join_conds.push(format!("{primary_alias}.{expr} = __nad.{name}"));
                    group_count += 1;
                }

                // When the query groups by metric_time, partition the NAD subquery
                // by the query's time granularity so MIN/MAX is computed per period.
                let query_time_gran = spec.group_by.iter().find_map(|gb| {
                    if let GroupBySpec::TimeDimension {
                        name, granularity, ..
                    } = gb
                    {
                        if name == "metric_time" || name == nad_name {
                            return Some(granularity.clone());
                        }
                    }
                    None
                });
                if let Some(ref gran) = query_time_gran {
                    let time_trunc = format!("DATE_TRUNC('{gran}', {nad_expr})");
                    nad_select.push(format!("{time_trunc} AS __nad_metric_time"));
                    nad_join_conds.push(format!(
                        "DATE_TRUNC('{gran}', {primary_alias}.{nad_expr}) = __nad.__nad_metric_time"
                    ));
                    group_count += 1;
                }

                let trunc = format!("DATE_TRUNC('{nad_gran}', {nad_expr})");
                nad_select.push(format!("{agg_fn}({trunc}) AS __nad_time"));
                nad_join_conds.push(format!(
                    "DATE_TRUNC('{nad_gran}', {primary_alias}.{nad_expr}) = __nad.__nad_time"
                ));

                let nad_from = render_full_relation(primary_model, dialect);

                // Build WHERE for the NAD subquery: include only filters that
                // reference the primary table. Filters on joined dimensions
                // (e.g., d1.home_state_latest) are excluded since the NAD
                // subquery is a standalone `SELECT ... FROM table`.
                let strip_alias = format!("{}.", primary_alias);
                let mut nad_where_parts: Vec<String> = Vec::new();
                for filter in metric
                    .metric_filters
                    .iter()
                    .chain(spec.where_filters.iter())
                {
                    let resolved =
                        resolve_where_filter(filter, model_aliases, dialect, &primary_model_name);
                    // Skip filters that reference joined table aliases (d1., d2., etc.)
                    let has_joined_ref = resolved.contains("d1.")
                        || resolved.contains("d2.")
                        || resolved.contains("D1.")
                        || resolved.contains("D2.");
                    if has_joined_ref {
                        continue;
                    }
                    nad_where_parts.push(resolved.replace(&strip_alias, ""));
                }
                if let Some((start, end)) = &spec.time_constraint {
                    nad_where_parts.push(format!(
                        "CAST({nad_expr} AS TIMESTAMP) >= CAST('{start}' AS TIMESTAMP)"
                    ));
                    nad_where_parts.push(format!(
                        "CAST({nad_expr} AS TIMESTAMP) < CAST('{end}' AS TIMESTAMP) + INTERVAL '1 day'"
                    ));
                }
                let nad_where = if nad_where_parts.is_empty() {
                    String::new()
                } else {
                    format!(" WHERE {}", nad_where_parts.join(" AND "))
                };

                let group_by = if group_count == 0 {
                    String::new()
                } else {
                    let indices: Vec<String> = (1..=group_count).map(|i| i.to_string()).collect();
                    format!(" GROUP BY {}", indices.join(", "))
                };

                let _ = writeln!(
                    sql,
                    "INNER JOIN (SELECT {} FROM {nad_from}{nad_where}{group_by}) AS __nad ON {}",
                    nad_select.join(", "),
                    nad_join_conds.join(" AND "),
                );
            }
        }
    }

    // Metric filter CTEs: detect {{ Metric(...) }} in WHERE filters, compile as CTEs,
    // and LEFT JOIN them to the main query.
    let all_filters: Vec<String> = metrics
        .iter()
        .flat_map(|m| m.metric_filters.iter().cloned())
        .chain(spec.where_filters.iter().cloned())
        .collect();
    let metric_filter_refs = extract_metric_filter_refs(&all_filters);
    let mut metric_filter_ctes: Vec<(String, String)> = Vec::new();
    let metric_filter_joins = compile_metric_filter_ctes(
        &metric_filter_refs,
        all_metrics,
        model_aliases,
        &primary_model_name,
        primary_alias,
        dialect,
        &mut metric_filter_ctes,
        join_edges,
    );
    for join_clause in &metric_filter_joins {
        let _ = writeln!(sql, "{join_clause}");
    }

    // WHERE clause: metric filters + user filters.
    // When use_case_when is active, metric-level filters are already in the CASE WHEN;
    // only add them to WHERE when all metrics share the same filters.
    let mut where_parts: Vec<String> = Vec::new();

    if !use_case_when {
        for metric in metrics {
            for filter in &metric.metric_filters {
                let resolved = resolve_where_filter_custom_gran(
                    filter,
                    model_aliases,
                    dialect,
                    &primary_model_name,
                    all_time_spines,
                    shared_agg_time_dim,
                    &mut custom_gran_joins,
                    &mut cg_alias_counter,
                    &mut sql,
                );
                where_parts.push(resolved);
            }
        }
    }

    for filter in &spec.where_filters {
        let resolved = resolve_where_filter_custom_gran(
            filter,
            model_aliases,
            dialect,
            &primary_model_name,
            all_time_spines,
            shared_agg_time_dim,
            &mut custom_gran_joins,
            &mut cg_alias_counter,
            &mut sql,
        );
        where_parts.push(resolved);
    }

    // Time constraint: filter on the raw agg_time_dimension column using
    // TIMESTAMP comparisons (>= start, < end+1day) to match MetricFlow semantics.
    if let Some((start, end)) = &spec.time_constraint {
        let raw_time_col =
            resolve_raw_time_column("metric_time", model_aliases, &primary_model_name);
        where_parts.push(format!(
            "CAST({raw_time_col} AS TIMESTAMP) >= CAST('{start}' AS TIMESTAMP)"
        ));
        where_parts.push(format!(
            "CAST({raw_time_col} AS TIMESTAMP) < CAST('{end}' AS TIMESTAMP) + INTERVAL '1 day'"
        ));
    }

    // Emit custom granularity JOINs (from both group-by and WHERE filter processing).
    for (cg_alias, cg_relation, cg_on) in &custom_gran_joins {
        let _ = writeln!(sql, "LEFT OUTER JOIN {cg_relation} {cg_alias} ON {cg_on}");
    }

    if !where_parts.is_empty() {
        let _ = writeln!(sql, "WHERE {}", where_parts.join("\n  AND "));
    }

    // GROUP BY.
    if !spec.group_by.is_empty() {
        let group_indices: Vec<String> = (1..=spec.group_by.len()).map(|i| i.to_string()).collect();
        let _ = writeln!(sql, "GROUP BY {}", group_indices.join(", "));
    }

    // ORDER BY.
    if !spec.order_by.is_empty() {
        let order_parts: Vec<String> = spec
            .order_by
            .iter()
            .map(|o| {
                let col = resolve_order_by_col(&o.name, &spec.group_by);
                if o.descending {
                    format!("{col} DESC")
                } else {
                    format!("{col} ASC")
                }
            })
            .collect();
        let _ = writeln!(sql, "ORDER BY {}", order_parts.join(", "));
    }

    // LIMIT.
    if let Some(limit) = spec.limit {
        let _ = writeln!(sql, "LIMIT {limit}");
    }

    // Prepend WITH clause if metric filter CTEs were generated.
    if !metric_filter_ctes.is_empty() {
        let mut with_sql = String::from("WITH\n");
        for (i, (name, cte_sql)) in metric_filter_ctes.iter().enumerate() {
            if i > 0 {
                with_sql.push_str(",\n");
            }
            let _ = write!(with_sql, "  {name} AS (\n    {cte_sql}\n  )");
        }
        with_sql.push('\n');
        with_sql.push_str(&sql);
        return Ok(with_sql.trim_end().to_string());
    }

    Ok(sql.trim_end().to_string())
}

// ═══════════════════════════════════════════════════════════════════════════
// Complex metric compilation (CTE-based path)
// ═══════════════════════════════════════════════════════════════════════════

#[allow(clippy::too_many_arguments)]
fn compile_complex_metrics(
    store: &mut impl MetricStore,
    spec: &SemanticQuerySpec,
    all_metrics: &HashMap<String, ResolvedMetric>,
    model_aliases: &HashMap<String, (String, &ResolvedModel)>,
    resolved_models: &HashMap<String, ResolvedModel>,
    join_edges: &[JoinEdge],
    dialect: Dialect,
    ctes: &mut Vec<(String, String)>,
    _final_select_columns: &mut Vec<String>,
    _final_from: &mut String,
    _final_joins: &mut Vec<String>,
) -> Result<String, MetricFlowError> {
    // Strategy: compile each top-level metric into a CTE, then combine.
    // Simple metrics: one CTE with aggregation.
    // Derived metrics: CTEs for each input, then a final CTE with the expression.
    // Ratio metrics: CTEs for numerator and denominator.
    // Cumulative metrics: CTE with window function.
    // Conversion metrics: CTE with self-join.

    let time_spine = load_time_spine(store);
    let all_time_spines = load_all_time_spines_from_store(store);

    // Find the coarsest metric-level time_granularity across all top-level metrics.
    let coarsest_metric_gran: Option<String> = spec
        .metrics
        .iter()
        .filter_map(|name| all_metrics.get(name))
        .filter_map(|m| m.time_granularity.clone())
        .max_by_key(|g| granularity_rank(g));

    for metric_name in &spec.metrics {
        let metric = all_metrics
            .get(metric_name)
            .ok_or_else(|| MetricFlowError::Other(format!("metric not found: {metric_name}")))?;

        // If any metric has a time_granularity coarser than the query's, create a
        // modified spec with clamped granularities and expanded time constraint.
        // All metrics in this multi-metric query use the same (coarsest) grain.
        let metric_spec;
        let effective_spec = if let Some(ref coarsest) = coarsest_metric_gran {
            let coarsest_query = spec
                .group_by
                .iter()
                .filter_map(|gb| {
                    if let GroupBySpec::TimeDimension { granularity, .. } = gb {
                        Some(granularity.as_str())
                    } else {
                        None
                    }
                })
                .max_by_key(|g| granularity_rank(g))
                .unwrap_or("day");
            if granularity_rank(coarsest) > granularity_rank(coarsest_query) {
                let mut s = spec.clone();
                for gb in &mut s.group_by {
                    if let GroupBySpec::TimeDimension {
                        name, granularity, ..
                    } = gb
                    {
                        if name == "metric_time"
                            && granularity_rank(coarsest) > granularity_rank(granularity)
                        {
                            *granularity = coarsest.clone();
                        }
                    }
                }
                if let Some((ref start, ref end)) = s.time_constraint {
                    let (new_start, new_end) =
                        expand_time_constraint_to_granularity(start, end, coarsest);
                    s.time_constraint = Some((new_start, new_end));
                }
                metric_spec = s;
                &metric_spec
            } else {
                spec
            }
        } else {
            spec
        };

        // For multi-metric queries where the outer WHERE won't apply the time
        // constraint (non-metric-time output dimension), push the time constraint
        // into each top-level simple metric CTE via the spec's where_filters.
        let has_time_gb = effective_spec
            .group_by
            .iter()
            .any(|gb| matches!(gb, GroupBySpec::TimeDimension { .. }));
        let outer_time_col_name = group_by_output_cols(&spec.group_by)
            .iter()
            .find(|c| c.contains("metric_time") || c.contains("__ds"))
            .cloned();
        let is_metric_time_col = outer_time_col_name
            .as_ref()
            .is_some_and(|c| c.contains("metric_time"));
        let need_cte_time_filter = has_time_gb
            && spec.metrics.len() > 1
            && !is_metric_time_col
            && effective_spec.time_constraint.is_some();

        match metric.metric_type {
            MetricType::Simple => {
                let cte_spec;
                let cte_effective_spec = if need_cte_time_filter {
                    let (start, end) = effective_spec.time_constraint.as_ref().unwrap();
                    let raw_time_col = resolve_raw_time_column(
                        "metric_time",
                        model_aliases,
                        metric
                            .agg_params
                            .as_ref()
                            .map(|ap| ap.semantic_model.as_str())
                            .unwrap_or(""),
                    );
                    let mut s = effective_spec.clone();
                    s.where_filters.push(format!(
                        "CAST({raw_time_col} AS TIMESTAMP) >= CAST('{start}' AS TIMESTAMP)"
                    ));
                    s.where_filters.push(format!(
                        "CAST({raw_time_col} AS TIMESTAMP) < CAST('{end}' AS TIMESTAMP) + INTERVAL '1 day'"
                    ));
                    cte_spec = s;
                    &cte_spec
                } else {
                    effective_spec
                };
                compile_simple_metric_cte(
                    metric,
                    cte_effective_spec,
                    all_metrics,
                    model_aliases,
                    join_edges,
                    &all_time_spines,
                    dialect,
                    "",
                    ctes,
                )?;
            }
            MetricType::Derived => {
                compile_derived_metric_cte(
                    metric,
                    spec,
                    all_metrics,
                    model_aliases,
                    join_edges,
                    dialect,
                    "",
                    ctes,
                    &all_time_spines,
                )?;
            }
            MetricType::Ratio => {
                compile_ratio_metric_cte(
                    metric,
                    spec,
                    all_metrics,
                    model_aliases,
                    join_edges,
                    &all_time_spines,
                    dialect,
                    "",
                    ctes,
                )?;
            }
            MetricType::Cumulative => {
                compile_cumulative_metric_cte(
                    metric,
                    spec,
                    all_metrics,
                    model_aliases,
                    join_edges,
                    dialect,
                    time_spine.as_ref(),
                    ctes,
                    &all_time_spines,
                )?;
            }
            MetricType::Conversion => {
                compile_conversion_metric_cte(
                    metric,
                    spec,
                    all_metrics,
                    model_aliases,
                    resolved_models,
                    join_edges,
                    dialect,
                    ctes,
                    &all_time_spines,
                )?;
            }
        }
    }

    // Build the final SQL from CTEs.
    build_final_sql(
        spec,
        ctes,
        dialect,
        all_metrics,
        time_spine.as_ref(),
        &all_time_spines,
    )
}

#[allow(clippy::too_many_arguments, clippy::cognitive_complexity)]
fn compile_simple_metric_cte(
    metric: &ResolvedMetric,
    spec: &SemanticQuerySpec,
    all_metrics: &HashMap<String, ResolvedMetric>,
    model_aliases: &HashMap<String, (String, &ResolvedModel)>,
    join_edges: &[JoinEdge],
    all_time_spines: &[TimeSpine],
    dialect: Dialect,
    cte_scope: &str,
    ctes: &mut Vec<(String, String)>,
) -> Result<(), MetricFlowError> {
    let cte_name = if cte_scope.is_empty() {
        metric.name.clone()
    } else {
        format!("{cte_scope}__{}", metric.name)
    };
    // Check if this CTE already exists (might be shared by multiple derived metrics).
    if ctes.iter().any(|(name, _)| *name == cte_name) {
        return Ok(());
    }

    let ap = metric.agg_params.as_ref().ok_or_else(|| {
        MetricFlowError::Other(format!(
            "simple metric {} has no aggregation params",
            metric.name
        ))
    })?;

    let (primary_alias, primary_model) =
        model_aliases.get(&ap.semantic_model).ok_or_else(|| {
            MetricFlowError::Other(format!(
                "semantic model not resolved: {}",
                ap.semantic_model
            ))
        })?;

    let mut cte_sql = String::new();
    let mut select_parts: Vec<String> = Vec::new();
    let out_cols = group_by_output_cols(&spec.group_by);

    let metric_agg_time_dim = ap.agg_time_dimension.as_deref();

    // Plan multi-hop subquery joins (for chains like account_id__customer_id
    // with dimensions from different leaf models).
    let mh_subqueries = plan_multi_hop_joins(
        spec,
        &ap.semantic_model,
        model_aliases,
        join_edges,
        dialect,
        &metric.metric_filters,
    );

    let mut custom_gran_joins: Vec<(String, String, String)> = Vec::new();
    let mut cg_alias_counter = 0u32;

    // Group-by columns.
    for (gb, out_col) in spec.group_by.iter().zip(out_cols.iter()) {
        match gb {
            GroupBySpec::TimeDimension {
                name,
                granularity,
                date_part,
            } => {
                if !is_standard_granularity(granularity) {
                    if let Some((spine, custom_col)) =
                        find_custom_granularity_spine(all_time_spines, granularity)
                    {
                        let cg_alias = if cg_alias_counter == 0 {
                            "ts_cg".to_string()
                        } else {
                            format!("ts_cg{cg_alias_counter}")
                        };
                        cg_alias_counter += 1;
                        let time_expr = resolve_time_dimension_ref_with_agg(
                            name,
                            "day",
                            model_aliases,
                            dialect,
                            &ap.semantic_model,
                            metric_agg_time_dim,
                        );
                        let spine_rel = match dialect {
                            Dialect::Databricks => spine.relation_name.replace('"', "`"),
                            _ => spine.relation_name.clone(),
                        };
                        let on_cond = format!("{time_expr} = {cg_alias}.{}", spine.primary_column);
                        custom_gran_joins.push((cg_alias.clone(), spine_rel, on_cond));
                        let col_expr = format!("{cg_alias}.{custom_col}");
                        if let Some(part) = date_part {
                            let extract_expr = render_extract(part, &col_expr, dialect);
                            select_parts.push(format!("{extract_expr} AS {out_col}"));
                        } else {
                            select_parts.push(format!("{col_expr} AS {out_col}"));
                        }
                    } else {
                        return Err(MetricFlowError::Other(format!(
                            "unknown custom granularity: {granularity}"
                        )));
                    }
                } else {
                    let col_expr = resolve_time_dimension_ref_with_agg(
                        name,
                        granularity,
                        model_aliases,
                        dialect,
                        &ap.semantic_model,
                        metric_agg_time_dim,
                    );
                    if let Some(part) = date_part {
                        let extract_expr = render_extract(part, &col_expr, dialect);
                        select_parts.push(format!("{extract_expr} AS {out_col}"));
                    } else {
                        select_parts.push(format!("{col_expr} AS {out_col}"));
                    }
                }
            }
            GroupBySpec::Dimension { entity, name } => {
                let dim_ref = match entity {
                    Some(e) => format!("{e}__{name}"),
                    None => name.clone(),
                };
                let col_expr = resolve_dimension_ref_with_mh(
                    &dim_ref,
                    model_aliases,
                    dialect,
                    &ap.semantic_model,
                    &mh_subqueries,
                );
                select_parts.push(format!("{col_expr} AS {out_col}"));
            }
            GroupBySpec::Entity { name } => {
                let col_expr = resolve_entity_ref(name, model_aliases, &ap.semantic_model);
                select_parts.push(format!("{col_expr} AS {out_col}"));
            }
        }
    }

    // Aggregation.
    let col_expr = qualify_measure_expr(primary_alias, &ap.expr);
    let agg_expr = render_agg_with_params(
        &ap.agg,
        &col_expr,
        dialect,
        ap.percentile,
        ap.use_discrete_percentile,
    );
    select_parts.push(format!("{agg_expr} AS {}", metric.name));

    let _ = write!(cte_sql, "SELECT {}", select_parts.join(", "));

    let from_relation = render_full_relation(primary_model, dialect);
    let _ = write!(cte_sql, " FROM {from_relation} AS {primary_alias}");

    // Compute the qualified fact-table time expression for SCD joins.
    let fact_time_expr: Option<String> = ap.agg_time_dimension.as_deref().and_then(|atd| {
        primary_model
            .dimensions
            .iter()
            .find(|d| d.name == atd)
            .map(|d| format!("{primary_alias}.{}", d.expr))
    });

    // Add joins for dimensions from other models.
    add_dimension_joins(
        spec,
        &metric.metric_filters,
        &ap.semantic_model,
        primary_alias,
        model_aliases,
        join_edges,
        dialect,
        &mut cte_sql,
        &mh_subqueries,
        fact_time_expr.as_deref(),
    );

    // Semi-additive measure handling: INNER JOIN to a subquery that filters on
    // MIN/MAX of the non-additive time dimension.
    if let Some(ref nad) = ap.non_additive_dimension {
        let nad_name = nad.get("name").and_then(|v| v.as_str()).unwrap_or("ds");
        let window_choice = nad
            .get("window_choice")
            .and_then(|v| v.as_str())
            .unwrap_or("max");
        let window_groupings: Vec<&str> = nad
            .get("window_groupings")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
            .unwrap_or_default();

        // Resolve the non-additive dimension's expression in the source table.
        let nad_dim = primary_model.dimensions.iter().find(|d| d.name == nad_name);
        let nad_expr = nad_dim.map(|d| d.expr.as_str()).unwrap_or(nad_name);
        let nad_granularity = nad_dim
            .and_then(|d| d.time_granularity.as_deref())
            .unwrap_or("day");

        let agg_fn = if window_choice == "min" { "MIN" } else { "MAX" };

        // Resolve window_grouping entity expressions.
        let grouping_exprs: Vec<(String, String)> = window_groupings
            .iter()
            .filter_map(|&g| {
                primary_model
                    .entities
                    .iter()
                    .find(|e| e.name == g)
                    .map(|e| (g.to_string(), e.expr.clone()))
            })
            .collect();

        // Build the subquery.
        let mut nad_select = Vec::new();
        let mut nad_join_conds = Vec::new();
        let mut group_count = 0usize;
        for (name, expr) in &grouping_exprs {
            nad_select.push(format!("{expr} AS {name}"));
            nad_join_conds.push(format!("{primary_alias}.{expr} = __nad.{name}"));
            group_count += 1;
        }

        // Partition by query time granularity when metric_time is in group_by.
        let query_time_gran = spec.group_by.iter().find_map(|gb| {
            if let GroupBySpec::TimeDimension {
                name, granularity, ..
            } = gb
            {
                if name == "metric_time" || name == nad_name {
                    return Some(granularity.clone());
                }
            }
            None
        });
        if let Some(ref gran) = query_time_gran {
            let time_trunc = format!("DATE_TRUNC('{gran}', {nad_expr})");
            nad_select.push(format!("{time_trunc} AS __nad_metric_time"));
            nad_join_conds.push(format!(
                "DATE_TRUNC('{gran}', {primary_alias}.{nad_expr}) = __nad.__nad_metric_time"
            ));
            group_count += 1;
        }

        let trunc = format!("DATE_TRUNC('{nad_granularity}', {nad_expr})");
        nad_select.push(format!("{agg_fn}({trunc}) AS __nad_time"));
        nad_join_conds.push(format!(
            "DATE_TRUNC('{nad_granularity}', {primary_alias}.{nad_expr}) = __nad.__nad_time"
        ));

        let nad_from = render_full_relation(primary_model, dialect);

        // Build WHERE clause for the NAD subquery: only include filters that
        // reference the primary table. Filters on joined dimensions are excluded
        // since the NAD subquery is a standalone `SELECT ... FROM table`.
        let strip_alias = format!("{}.", primary_alias);
        let mut nad_where_parts: Vec<String> = Vec::new();
        for filter in metric
            .metric_filters
            .iter()
            .chain(spec.where_filters.iter())
        {
            let resolved = resolve_where_filter(filter, model_aliases, dialect, &ap.semantic_model);
            let has_joined_ref = resolved.contains("d1.")
                || resolved.contains("d2.")
                || resolved.contains("D1.")
                || resolved.contains("D2.");
            if has_joined_ref {
                continue;
            }
            nad_where_parts.push(resolved.replace(&strip_alias, ""));
        }
        if let Some((start, end)) = &spec.time_constraint {
            nad_where_parts.push(format!(
                "CAST({nad_expr} AS TIMESTAMP) >= CAST('{start}' AS TIMESTAMP)"
            ));
            nad_where_parts.push(format!(
                "CAST({nad_expr} AS TIMESTAMP) < CAST('{end}' AS TIMESTAMP) + INTERVAL '1 day'"
            ));
        }
        let nad_where = if nad_where_parts.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", nad_where_parts.join(" AND "))
        };

        let group_by = if group_count == 0 {
            String::new()
        } else {
            let indices: Vec<String> = (1..=group_count).map(|i| i.to_string()).collect();
            format!(" GROUP BY {}", indices.join(", "))
        };

        let _ = write!(
            cte_sql,
            " INNER JOIN (SELECT {} FROM {nad_from}{nad_where}{group_by}) AS __nad ON {}",
            nad_select.join(", "),
            nad_join_conds.join(" AND "),
        );
    }

    // Metric filter CTEs: if any metric_filter references {{ Metric(...) }}, emit the
    // corresponding __mf_* CTE before this one and JOIN it into the CTE body so that
    // the WHERE reference resolves correctly.
    let mut mf_refs = extract_metric_filter_refs(&metric.metric_filters);
    mf_refs.extend(extract_metric_filter_refs(&spec.where_filters));
    if !mf_refs.is_empty() {
        let mf_joins = compile_metric_filter_ctes(
            &mf_refs,
            all_metrics,
            model_aliases,
            &ap.semantic_model,
            primary_alias,
            dialect,
            ctes,
            join_edges,
        );
        for join in &mf_joins {
            let _ = write!(cte_sql, " {join}");
        }
    }

    // WHERE: metric-level filters + user-supplied spec.where_filters + time constraint.
    // Resolve filters first (may add custom granularity joins), then emit JOINs, then WHERE.
    let mut where_parts: Vec<String> = Vec::new();
    let all_filters = metric
        .metric_filters
        .iter()
        .chain(spec.where_filters.iter());
    for filter in all_filters {
        let resolved = resolve_where_filter_custom_gran(
            filter,
            model_aliases,
            dialect,
            &ap.semantic_model,
            all_time_spines,
            metric_agg_time_dim,
            &mut custom_gran_joins,
            &mut cg_alias_counter,
            &mut cte_sql,
        );
        where_parts.push(resolved);
    }
    // Push time constraint into CTE only when no time dimension in group_by
    // (outer query can't apply it).
    let has_time_gb = spec
        .group_by
        .iter()
        .any(|gb| matches!(gb, GroupBySpec::TimeDimension { .. }));
    if !has_time_gb {
        if let Some((start, end)) = &spec.time_constraint {
            let raw_time_col =
                resolve_raw_time_column("metric_time", model_aliases, &ap.semantic_model);
            where_parts.push(format!(
                "CAST({raw_time_col} AS TIMESTAMP) >= CAST('{start}' AS TIMESTAMP)"
            ));
            where_parts.push(format!(
                "CAST({raw_time_col} AS TIMESTAMP) < CAST('{end}' AS TIMESTAMP) + INTERVAL '1 day'"
            ));
        }
    }

    for (cg_alias, cg_relation, cg_on) in &custom_gran_joins {
        let _ = write!(
            cte_sql,
            " LEFT OUTER JOIN {cg_relation} {cg_alias} ON {cg_on}"
        );
    }

    if !where_parts.is_empty() {
        let _ = write!(cte_sql, " WHERE {}", where_parts.join(" AND "));
    }

    // GROUP BY.
    if !spec.group_by.is_empty() {
        let group_indices: Vec<String> = (1..=spec.group_by.len()).map(|i| i.to_string()).collect();
        let _ = write!(cte_sql, " GROUP BY {}", group_indices.join(", "));
    }

    ctes.push((cte_name, cte_sql));
    Ok(())
}

#[allow(clippy::cognitive_complexity)]
#[allow(clippy::too_many_arguments)]
fn compile_derived_metric_cte(
    metric: &ResolvedMetric,
    spec: &SemanticQuerySpec,
    all_metrics: &HashMap<String, ResolvedMetric>,
    model_aliases: &HashMap<String, (String, &ResolvedModel)>,
    join_edges: &[JoinEdge],
    dialect: Dialect,
    cte_scope: &str,
    ctes: &mut Vec<(String, String)>,
    all_time_spines: &[TimeSpine],
) -> Result<(), MetricFlowError> {
    // Check if this derived metric CTE already exists (shared by multiple inputs).
    let scoped_name = if cte_scope.is_empty() {
        metric.name.clone()
    } else {
        format!("{cte_scope}__{}", metric.name)
    };
    if ctes.iter().any(|(name, _)| *name == scoped_name) {
        return Ok(());
    }

    // Propagate any filters defined on this derived metric down to its inputs.
    // When filters are present we assign a unique CTE-name scope so that two
    // outer derived metrics with different filters each get their own copies of
    // shared base CTEs rather than incorrectly reusing the first one's filtered CTE.
    let (child_spec, child_scope) = if metric.metric_filters.is_empty() {
        (spec.clone(), cte_scope.to_string())
    } else {
        let mut s = spec.clone();
        s.where_filters
            .extend(metric.metric_filters.iter().cloned());
        (s, metric.name.clone())
    };
    let child_scope = child_scope.as_str();

    // When a derived metric has offset inputs AND the spec requests a non-day
    // granularity, base CTEs must be compiled at a uniform granularity so the
    // offset join (spine.time - INTERVAL = base.time) lines up correctly.
    // For daily-or-coarser queries we normalize to "day"; for subdaily queries
    // we keep the query's finer granularity (e.g. "hour").
    let has_offsets = metric
        .input_metrics
        .iter()
        .any(|i| i.offset_window.is_some() || i.offset_to_grain.is_some());
    let normalized_spec;
    let base_spec = if has_offsets {
        let base_gran = child_spec
            .group_by
            .iter()
            .find_map(|gb| {
                if let GroupBySpec::TimeDimension { granularity, .. } = gb {
                    if matches!(granularity.as_str(), "hour" | "minute" | "second") {
                        return Some(granularity.as_str());
                    }
                }
                None
            })
            .unwrap_or("day");
        let mut seen_time_dims: HashSet<String> = HashSet::new();
        let mut norm_group_by: Vec<GroupBySpec> = child_spec
            .group_by
            .iter()
            .filter_map(|gb| match gb {
                GroupBySpec::TimeDimension { name, .. } => {
                    if seen_time_dims.insert(name.clone()) {
                        Some(GroupBySpec::TimeDimension {
                            name: name.clone(),
                            granularity: base_gran.into(),
                            date_part: None,
                        })
                    } else {
                        None
                    }
                }
                other => Some(other.clone()),
            })
            .collect();
        // Extract Dimension('entity__name') refs from WHERE filters that are NOT
        // already in group_by — these need to be carried through base CTEs for
        // WHERE filtering, then stripped from the final output.
        let existing_dim_cols: HashSet<String> = norm_group_by
            .iter()
            .filter_map(|gb| match gb {
                GroupBySpec::Dimension {
                    entity: Some(e),
                    name,
                } => Some(format!("{e}__{name}")),
                GroupBySpec::Dimension { entity: None, name } => Some(name.clone()),
                _ => None,
            })
            .collect();
        for filter in child_spec
            .where_filters
            .iter()
            .chain(metric.metric_filters.iter())
        {
            let mut cursor = 0usize;
            while let Some(start) = filter[cursor..].find("Dimension('") {
                let abs_pos = cursor + start;
                // Skip TimeDimension('...') — only match standalone Dimension('...')
                if abs_pos >= 4 && filter[abs_pos.saturating_sub(4)..abs_pos] == *"Time" {
                    cursor = abs_pos + 11;
                    continue;
                }
                let abs_start = abs_pos + 11;
                if let Some(end) = filter[abs_start..].find("')") {
                    let dim_ref = &filter[abs_start..abs_start + end];
                    if !existing_dim_cols.contains(dim_ref) {
                        if let Some((entity, name)) = dim_ref.split_once("__") {
                            if !norm_group_by.iter().any(|gb| matches!(gb, GroupBySpec::Dimension { entity: Some(e), name: n } if e == entity && n == name)) {
                                norm_group_by.push(GroupBySpec::Dimension {
                                    entity: Some(entity.to_string()),
                                    name: name.to_string(),
                                });
                            }
                        } else if !norm_group_by.iter().any(|gb| matches!(gb, GroupBySpec::Dimension { entity: None, name: n } if n == dim_ref)) {
                            norm_group_by.push(GroupBySpec::Dimension {
                                entity: None,
                                name: dim_ref.to_string(),
                            });
                        }
                    }
                    cursor = abs_start + end + 2;
                } else {
                    break;
                }
            }
        }
        normalized_spec = SemanticQuerySpec {
            metrics: child_spec.metrics.clone(),
            group_by: norm_group_by,
            where_filters: Vec::new(),
            order_by: child_spec.order_by.clone(),
            limit: child_spec.limit,
            time_constraint: child_spec.time_constraint.clone(),
            apply_group_by: child_spec.apply_group_by,
        };
        &normalized_spec
    } else {
        &child_spec
    };

    // First, compile all input metrics as CTEs (using base_spec for day granularity).
    for input in &metric.input_metrics {
        let input_spec_owned;
        let input_scope_owned;
        let (effective_spec, effective_scope) = if input.filters.is_empty() {
            (base_spec, child_scope)
        } else {
            input_spec_owned = SemanticQuerySpec {
                where_filters: base_spec
                    .where_filters
                    .iter()
                    .cloned()
                    .chain(input.filters.iter().cloned())
                    .collect(),
                ..base_spec.clone()
            };
            let alias = input.alias.as_deref().unwrap_or(&input.name);
            input_scope_owned = format!("{child_scope}__{alias}");
            (&input_spec_owned, input_scope_owned.as_str())
        };
        if let Some(input_metric) = all_metrics.get(&input.name) {
            match input_metric.metric_type {
                MetricType::Simple => {
                    compile_simple_metric_cte(
                        input_metric,
                        effective_spec,
                        all_metrics,
                        model_aliases,
                        join_edges,
                        all_time_spines,
                        dialect,
                        effective_scope,
                        ctes,
                    )?;
                }
                MetricType::Derived => {
                    compile_derived_metric_cte(
                        input_metric,
                        effective_spec,
                        all_metrics,
                        model_aliases,
                        join_edges,
                        dialect,
                        effective_scope,
                        ctes,
                        all_time_spines,
                    )?;
                }
                MetricType::Ratio => {
                    compile_ratio_metric_cte(
                        input_metric,
                        effective_spec,
                        all_metrics,
                        model_aliases,
                        join_edges,
                        all_time_spines,
                        dialect,
                        effective_scope,
                        ctes,
                    )?;
                }
                MetricType::Cumulative => {
                    compile_cumulative_metric_cte(
                        input_metric,
                        effective_spec,
                        all_metrics,
                        model_aliases,
                        join_edges,
                        dialect,
                        None,
                        ctes,
                        all_time_spines,
                    )?;
                }
                _ => {
                    if input_metric.agg_params.is_some() {
                        compile_simple_metric_cte(
                            input_metric,
                            effective_spec,
                            all_metrics,
                            model_aliases,
                            join_edges,
                            all_time_spines,
                            dialect,
                            effective_scope,
                            ctes,
                        )?;
                    }
                }
            }
        }
    }

    // ── Spine-join wrappers for join_to_timespine inputs ─────────────
    // When an input metric has join_to_timespine=true, wrap its CTE in a
    // LEFT JOIN from the time spine table to fill gaps with COALESCE(..., fill_value).
    let query_gran = base_spec
        .group_by
        .iter()
        .find_map(|gb| {
            if let GroupBySpec::TimeDimension { granularity, .. } = gb {
                Some(granularity.as_str())
            } else {
                None
            }
        })
        .unwrap_or("day");
    if let Some(spine) = pick_time_spine_for_granularity(all_time_spines, query_gran) {
        let spine_relation = match dialect {
            Dialect::Databricks => spine.relation_name.replace('"', "`"),
            _ => spine.relation_name.clone(),
        };
        let has_time_for_spine = base_spec
            .group_by
            .iter()
            .any(|gb| matches!(gb, GroupBySpec::TimeDimension { .. }));
        for input in &metric.input_metrics {
            if let Some(input_metric) = all_metrics.get(&input.name) {
                if input_metric.join_to_timespine && has_time_for_spine {
                    let base_cte = if child_scope.is_empty() {
                        input.name.clone()
                    } else {
                        format!("{child_scope}__{}", input.name)
                    };
                    let raw_cte = format!("{base_cte}_raw");
                    if !ctes.iter().any(|(n, _)| *n == raw_cte) {
                        let time_col = base_spec
                            .group_by
                            .iter()
                            .find_map(|gb| {
                                if let GroupBySpec::TimeDimension { name, .. } = gb {
                                    Some(name.as_str())
                                } else {
                                    None
                                }
                            })
                            .unwrap_or("metric_time");
                        let fill = input_metric.fill_nulls_with.unwrap_or(0);
                        let trunc =
                            format!("DATE_TRUNC('{query_gran}', t.{})", spine.primary_column);
                        let mut sel = vec![format!("{trunc} AS {time_col}")];
                        for gb in &base_spec.group_by {
                            match gb {
                                GroupBySpec::Dimension {
                                    entity: Some(e),
                                    name,
                                } => {
                                    sel.push(format!("mc.{e}__{name}"));
                                }
                                GroupBySpec::Dimension { entity: None, name } => {
                                    sel.push(format!("mc.{name}"));
                                }
                                _ => {}
                            }
                        }
                        sel.push(format!(
                            "COALESCE(mc.{}, {fill}) AS {}",
                            input.name, input.name
                        ));
                        let spine_sql = format!(
                            "SELECT DISTINCT {} FROM {spine_relation} AS t LEFT JOIN {raw_cte} AS mc ON {trunc} = mc.{time_col}",
                            sel.join(", "),
                        );
                        // Rename the original CTE to _raw, add spine-joined under original name.
                        if let Some(pos) = ctes.iter().position(|(n, _)| *n == base_cte) {
                            ctes[pos].0 = raw_cte.clone();
                        }
                        ctes.push((base_cte.clone(), spine_sql));
                    }
                }
            }
        }
    }

    // ── Offset wrapper CTEs ────────────────────────────────────────────
    // For inputs with offset_window or offset_to_grain, create wrapper CTEs
    // that join the base metric CTE to an inline time spine with a shifted condition.
    // Track renamed wrappers so `effective_cte_name` can look up the right name.
    let mut renamed_wrappers: HashMap<String, String> = HashMap::new();
    let has_time_dim = spec
        .group_by
        .iter()
        .any(|gb| matches!(gb, GroupBySpec::TimeDimension { .. }));
    for input in &metric.input_metrics {
        if input.offset_window.is_none() && input.offset_to_grain.is_none() {
            continue;
        }
        let raw_alias = input.alias.as_deref().unwrap_or(&input.name);
        // When the alias collides with an already-emitted CTE name, use a distinct
        // wrapper name so we don't produce duplicate CTE names.
        let needs_rename = ctes.iter().any(|(name, _)| name == raw_alias);
        let alias = if needs_rename {
            let renamed = format!("{raw_alias}_offset");
            renamed_wrappers.insert(raw_alias.to_string(), renamed.clone());
            renamed
        } else {
            raw_alias.to_string()
        };
        let wrapper_spine = format!("{alias}_spine");
        if ctes.iter().any(|(name, _)| name == &wrapper_spine) {
            continue;
        }
        if !has_time_dim {
            return Err(MetricFlowError::Other(format!(
                "offset metric input '{}' requires a time dimension group-by",
                input.name
            )));
        }

        // Find the time dimension column name from spec.
        let time_col = spec
            .group_by
            .iter()
            .find_map(|gb| {
                if let GroupBySpec::TimeDimension { name, .. } = gb {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "metric_time__day".to_string());

        // Detect custom granularity in offset_window early so we can skip the
        // standard spine CTE for custom granularity offsets.
        let spine_name = format!("{alias}_spine");
        let custom_offset_gran = input.offset_window.as_ref().and_then(|ow| {
            let parts: Vec<&str> = ow.splitn(2, ' ').collect();
            if parts.len() == 2 {
                let gran = parts[1].trim_end_matches('s');
                if !is_standard_granularity(gran) {
                    let count: i64 = parts[0].parse().unwrap_or(1);
                    return Some((count, gran.to_string()));
                }
            }
            None
        });

        if let Some((offset_count, custom_gran)) = custom_offset_gran {
            // Custom granularity offset: use FIRST_VALUE/LAST_VALUE/ROW_NUMBER/LEAD pattern.
            let (spine, custom_col) = find_custom_granularity_spine(all_time_spines, &custom_gran)
                .ok_or_else(|| {
                    MetricFlowError::Other(format!(
                        "custom granularity '{}' not found on any time spine",
                        custom_gran
                    ))
                })?;
            let spine_rel = match dialect {
                Dialect::Databricks => spine.relation_name.replace('"', "`"),
                _ => spine.relation_name.clone(),
            };
            let ds_col = &spine.primary_column;

            // Build CTE: map each spine day to its custom gran bucket + position.
            let mapping_cte_name = format!("{alias}_cg_map");
            let mapping_sql = format!(
                "SELECT \
                 {ds_col} AS ds__day, \
                 {custom_col} AS ds__cg, \
                 FIRST_VALUE({ds_col}) OVER (PARTITION BY {custom_col} ORDER BY {ds_col} \
                   ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING) AS ds__cg__first_value, \
                 LAST_VALUE({ds_col}) OVER (PARTITION BY {custom_col} ORDER BY {ds_col} \
                   ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING) AS ds__cg__last_value, \
                 ROW_NUMBER() OVER (PARTITION BY {custom_col} ORDER BY {ds_col}) AS ds__day__row_number \
                 FROM {spine_rel} AS ts"
            );
            ctes.push((mapping_cte_name.clone(), mapping_sql));

            // Build offset mapping subquery using LEAD to shift by N custom-gran buckets.
            let row_offset_expr = format!("{mapping_cte_name}.ds__day__row_number - 1");
            let add_days = |base: &str| match dialect {
                Dialect::DuckDB => format!("{base} + INTERVAL (({row_offset_expr})) day"),
                _ => format!("DATEADD('DAY', {row_offset_expr}, {base})"),
            };
            let offset_first = add_days("lead_q.ds__cg__first_value__offset");
            let lead_subq = format!(
                "SELECT ds__day, \
                 CASE \
                   WHEN {offset_first} \
                     <= lead_q.ds__cg__last_value__offset \
                   THEN {offset_first} \
                   ELSE lead_q.ds__cg__last_value__offset \
                 END AS ds__day__lead \
                 FROM {mapping_cte_name} \
                 INNER JOIN (\
                   SELECT ds__cg, \
                     LEAD(ds__cg__first_value, {offset_count}) OVER (ORDER BY ds__cg) AS ds__cg__first_value__offset, \
                     LEAD(ds__cg__last_value, {offset_count}) OVER (ORDER BY ds__cg) AS ds__cg__last_value__offset \
                   FROM (\
                     SELECT ds__cg__first_value, ds__cg__last_value, ds__cg \
                     FROM {mapping_cte_name} \
                     GROUP BY ds__cg__first_value, ds__cg__last_value, ds__cg\
                   ) cg_distinct\
                 ) lead_q \
                 ON {mapping_cte_name}.ds__cg = lead_q.ds__cg"
            );

            // The offset wrapper: replaces the standard spine+base JOIN.
            // Select the lead day as the time column, carry through non-time dims and metric.
            let mut offset_select = format!("offset_map.ds__day__lead AS {time_col}");
            for gb in &base_spec.group_by {
                let col = match gb {
                    GroupBySpec::TimeDimension { .. } => continue,
                    GroupBySpec::Dimension {
                        entity: Some(e),
                        name,
                    } => format!("{e}__{name}"),
                    GroupBySpec::Dimension { entity: None, name } => name.clone(),
                    GroupBySpec::Entity { name } => name.clone(),
                };
                offset_select.push_str(&format!(", base.{col}"));
            }
            if alias == metric.name.as_str() {
                offset_select.push_str(&format!(", base.{} AS {}", input.name, metric.name));
            } else {
                offset_select.push_str(&format!(", base.{}", input.name));
            }

            let offset_sql = format!(
                "SELECT {offset_select} \
                 FROM ({lead_subq}) AS offset_map \
                 INNER JOIN {base} AS base ON offset_map.ds__day = base.{time_col}",
                base = input.name,
            );

            // Check if the outer spec needs re-aggregation (coarser gran, date_part, or
            // custom gran on top of the day-level offset output).
            let cg_needs_reagg = spec.group_by.iter().any(|gb| {
                matches!(gb, GroupBySpec::TimeDimension { granularity, date_part, .. }
                    if granularity.as_str() != "day" || date_part.is_some())
            });
            if cg_needs_reagg {
                let raw_name = format!("{alias}_cg_offset");
                ctes.push((raw_name.clone(), offset_sql));

                let group_by_cols_outer = group_by_output_cols(&spec.group_by);
                let metric_col = if alias == metric.name.as_str() {
                    metric.name.as_str()
                } else {
                    input.name.as_str()
                };
                let mut reagg_sel: Vec<String> = Vec::new();
                let mut reagg_grp: Vec<String> = Vec::new();
                let mut cg_join_reagg = String::new();
                for (gi, (gb, out_col)) in
                    (1usize..).zip(spec.group_by.iter().zip(group_by_cols_outer.iter()))
                {
                    match gb {
                        GroupBySpec::TimeDimension {
                            granularity,
                            date_part,
                            ..
                        } => {
                            let src = format!("CAST(raw.{time_col} AS TIMESTAMP)");
                            if let Some(part) = date_part {
                                reagg_sel.push(format!(
                                    "{} AS {out_col}",
                                    render_extract(part, &src, dialect)
                                ));
                            } else if !is_standard_granularity(granularity) {
                                if let Some((cg_spine, cg_col)) =
                                    find_custom_granularity_spine(all_time_spines, granularity)
                                {
                                    let cg_rel = match dialect {
                                        Dialect::Databricks => {
                                            cg_spine.relation_name.replace('"', "`")
                                        }
                                        _ => cg_spine.relation_name.clone(),
                                    };
                                    if cg_join_reagg.is_empty() {
                                        cg_join_reagg = format!(
                                            " LEFT OUTER JOIN {cg_rel} ts_reagg \
                                             ON raw.{time_col} = ts_reagg.{}",
                                            cg_spine.primary_column,
                                        );
                                    }
                                    reagg_sel.push(format!("ts_reagg.{cg_col} AS {out_col}"));
                                } else {
                                    reagg_sel.push(format!("raw.{time_col} AS {out_col}"));
                                }
                            } else {
                                let trunc = render_date_trunc(granularity, &src, dialect);
                                reagg_sel.push(format!("{trunc} AS {out_col}"));
                            }
                        }
                        _ => {
                            reagg_sel.push(format!("raw.{out_col}"));
                        }
                    }
                    reagg_grp.push(gi.to_string());
                }
                reagg_sel.push(format!("SUM(raw.{metric_col}) AS {}", metric.name));
                let reagg_sql = format!(
                    "SELECT {} FROM {raw_name} AS raw{cg_join_reagg} GROUP BY {}",
                    reagg_sel.join(", "),
                    reagg_grp.join(", "),
                );
                ctes.push((alias.to_string(), reagg_sql));
            } else {
                ctes.push((alias.to_string(), offset_sql));
            }
        } else {
            // Standard offset: use INTERVAL-based spine approach.
            // Generate inline time spine CTE.
            let spine_gran = base_spec
                .group_by
                .iter()
                .find_map(|gb| {
                    if let GroupBySpec::TimeDimension { granularity, .. } = gb {
                        Some(granularity.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("day");
            let subdaily_spine = matches!(spine_gran, "hour" | "minute" | "second");
            let spine_target_type = if subdaily_spine { "TIMESTAMP" } else { "DATE" };
            if !ctes.iter().any(|(name, _)| name == &spine_name) {
                let offset_extension = input
                    .offset_window
                    .as_ref()
                    .map(|w| render_interval_str(w, dialect));
                let offset_spine = pick_time_spine_for_granularity(all_time_spines, spine_gran);
                let spine_sql = if let Some(ts) = offset_spine {
                    let spine_rel = match dialect {
                        Dialect::Databricks => ts.relation_name.replace('"', "`"),
                        _ => ts.relation_name.clone(),
                    };
                    let cast = render_type_cast(
                        &format!("t.{}", ts.primary_column),
                        spine_target_type,
                        dialect,
                    );
                    if let Some(ref ext) = offset_extension {
                        format!(
                            "SELECT DISTINCT {cast} AS {time_col} FROM {spine_rel} AS t \
                             WHERE {cast} >= (SELECT MIN({time_col}) FROM {0}) \
                             AND {cast} <= (SELECT MAX({time_col}) FROM {0}) + {ext}",
                            input.name,
                        )
                    } else {
                        format!("SELECT DISTINCT {cast} AS {time_col} FROM {spine_rel} AS t",)
                    }
                } else if let Some(ref ext) = offset_extension {
                    let min_expr = format!("(SELECT MIN({time_col}) FROM {0})", input.name);
                    let max_expr = format!("(SELECT MAX({time_col}) FROM {0}) + {ext}", input.name);
                    match dialect {
                        Dialect::DuckDB => {
                            let cast = render_type_cast("ds", spine_target_type, dialect);
                            format!(
                                "SELECT {cast} AS {time_col} \
                                 FROM generate_series({min_expr}, {max_expr}, INTERVAL '1 {spine_gran}') AS t(ds)"
                            )
                        }
                        _ => inline_time_spine_sql(
                            &time_col,
                            &input.name,
                            &time_col,
                            spine_gran,
                            dialect,
                        ),
                    }
                } else {
                    inline_time_spine_sql(&time_col, &input.name, &time_col, spine_gran, dialect)
                };
                ctes.push((spine_name.clone(), spine_sql));
            }

            let join_condition = if let Some(ref offset_window) = input.offset_window {
                let interval = render_interval_str(offset_window, dialect);
                format!("spine.{time_col} - {interval} = base.{time_col}")
            } else if let Some(ref grain) = input.offset_to_grain {
                format!("DATE_TRUNC('{grain}', spine.{time_col}) = base.{time_col}")
            } else {
                unreachable!()
            };

            // Carry non-time group-by columns through the offset wrapper.
            let mut offset_select = format!("spine.{time_col}");
            for gb in &base_spec.group_by {
                let col = match gb {
                    GroupBySpec::TimeDimension { .. } => continue,
                    GroupBySpec::Dimension {
                        entity: Some(e),
                        name,
                    } => format!("{e}__{name}"),
                    GroupBySpec::Dimension { entity: None, name } => name.clone(),
                    GroupBySpec::Entity { name } => name.clone(),
                };
                offset_select.push_str(&format!(", base.{col}"));
            }
            if alias == metric.name.as_str() {
                offset_select.push_str(&format!(", base.{} AS {}", input.name, metric.name));
            } else {
                offset_select.push_str(&format!(", base.{}", input.name));
            }

            let offset_sql = format!(
                "SELECT {offset_select} \
                 FROM {spine_name} AS spine \
                 INNER JOIN {base} AS base ON {join_condition}",
                base = input.name,
            );
            ctes.push((alias.to_string(), offset_sql));
        }
    }

    // Now build the derived metric CTE that references the input CTEs.
    let expr = metric.derived_expr.as_deref().ok_or_else(|| {
        MetricFlowError::Other(format!("derived metric {} has no expression", metric.name))
    })?;

    // The expression references input metrics by name.
    // We need to join the input CTEs and substitute metric names with CTE column references.

    let group_by_cols = group_by_output_cols(&spec.group_by);
    // For JOIN conditions we need the base CTE column names (from base_spec), which
    // use deduplicated day-level names when offsets are present.
    let join_cols = if has_offsets {
        group_by_output_cols(&base_spec.group_by)
    } else {
        group_by_cols.clone()
    };
    // Dims added to base_spec for WHERE filtering but not in the original group_by.
    // Only non-time dimensions count (time dims differ by granularity, not by presence).
    let where_only_dims: Vec<String> = if has_offsets {
        let time_dim_names: HashSet<&str> = base_spec
            .group_by
            .iter()
            .filter_map(|gb| {
                if let GroupBySpec::TimeDimension { name, .. } = gb {
                    Some(name.as_str())
                } else {
                    None
                }
            })
            .collect();
        join_cols
            .iter()
            .filter(|c| !group_by_cols.contains(c) && !time_dim_names.contains(c.as_str()))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };

    if metric.input_metrics.is_empty() {
        return Err(MetricFlowError::Other(format!(
            "derived metric {} has no input metrics",
            metric.name
        )));
    }

    // Determine the effective CTE name for each input:
    // - If the input has an offset, the wrapper CTE is named after the alias.
    // - Otherwise, the CTE is the base metric name, scoped when child_scope is set.
    let effective_cte_name = |input: &MetricInput| -> String {
        let raw_alias = input.alias.as_deref().unwrap_or(&input.name);
        let base = if input.offset_window.is_some() || input.offset_to_grain.is_some() {
            if let Some(renamed) = renamed_wrappers.get(raw_alias) {
                renamed.clone()
            } else {
                raw_alias.to_string()
            }
        } else {
            input.name.clone()
        };
        if !input.filters.is_empty() {
            let alias = input.alias.as_deref().unwrap_or(&input.name);
            format!("{child_scope}__{alias}__{base}")
        } else if child_scope.is_empty() {
            base
        } else {
            format!("{child_scope}__{base}")
        }
    };

    let first_input = &metric.input_metrics[0];
    let first_cte_alias = format!(
        "{}_cte",
        first_input.alias.as_deref().unwrap_or(&first_input.name)
    );
    let first_effective_cte = effective_cte_name(first_input);

    let mut select_parts: Vec<String> = Vec::new();
    let mut derived_cg_join: Option<(String, String, String)> = None;
    for (gb, out_col) in spec.group_by.iter().zip(group_by_cols.iter()) {
        match gb {
            GroupBySpec::TimeDimension {
                name,
                granularity,
                date_part,
            } if has_offsets
                && ({
                    let base_gran = base_spec
                        .group_by
                        .iter()
                        .find_map(|g| {
                            if let GroupBySpec::TimeDimension {
                                granularity: bg, ..
                            } = g
                            {
                                Some(bg.as_str())
                            } else {
                                None
                            }
                        })
                        .unwrap_or("day");
                    granularity != base_gran || date_part.is_some()
                }) =>
            {
                // Base CTEs are at base_gran; apply truncation/extract in the outer CTE.
                let col = format!("{first_cte_alias}.{name}");
                if let Some(part) = date_part {
                    select_parts.push(format!(
                        "{} AS {out_col}",
                        render_extract(part, &col, dialect)
                    ));
                } else if let Some((cg_spine, cg_col)) =
                    find_custom_granularity_spine(all_time_spines, granularity)
                {
                    let spine_rel = match dialect {
                        Dialect::Databricks => cg_spine.relation_name.replace('"', "`"),
                        _ => cg_spine.relation_name.clone(),
                    };
                    derived_cg_join =
                        Some((spine_rel, cg_spine.primary_column.clone(), name.clone()));
                    select_parts.push(format!("ts_dcg.{cg_col} AS {out_col}"));
                } else {
                    let trunc = render_date_trunc(granularity, &col, dialect);
                    select_parts.push(format!("{trunc} AS {out_col}"));
                }
            }
            _ => {
                // Placeholder — will be replaced after all joins are known
                // so we can COALESCE across all CTE aliases for 3+ inputs.
                select_parts.push(format!("__GROUP_BY_PLACEHOLDER_{out_col}__"));
            }
        }
    }
    // Include WHERE-only dims so the deferred filter can reference them.
    for dim_col in &where_only_dims {
        select_parts.push(format!("__GROUP_BY_PLACEHOLDER_{dim_col}__"));
    }

    // Build expression with CTE references.
    // The expression uses the alias (or metric name if no alias) to refer to
    // each input metric.  The CTE's output column is the metric's actual name.
    // Two-pass replacement to avoid cascading matches (e.g. replacing "bookings"
    // inside an already-substituted "bookings_1_day_ago_cte.bookings").
    let mut resolved_expr = expr.to_string();
    let mut replacements: Vec<(String, String, String)> = metric
        .input_metrics
        .iter()
        .enumerate()
        .map(|(i, input)| {
            let expr_ref = input.alias.as_deref().unwrap_or(&input.name);
            let cte_alias = format!("{}_cte", expr_ref);
            let placeholder = format!("\x00PH{i}\x00");
            let col_ref = format!("{cte_alias}.{}", input.name);
            // If the input metric has fill_nulls_with, wrap in COALESCE so that
            // NULL values from FULL OUTER JOIN gaps get filled.
            let final_ref =
                if let Some(fill) = all_metrics.get(&input.name).and_then(|m| m.fill_nulls_with) {
                    format!("COALESCE({col_ref}, {fill})")
                } else {
                    col_ref
                };
            (expr_ref.to_string(), placeholder, final_ref)
        })
        .collect();
    // Sort longest-first to prefer longer matches.
    replacements.sort_by_key(|a| std::cmp::Reverse(a.0.len()));
    // Pass 1: replace word matches with unique placeholders.
    for (find, placeholder, _) in &replacements {
        resolved_expr = replace_word(&resolved_expr, find, placeholder);
    }
    // Pass 2: replace placeholders with final values.
    for (_, placeholder, final_val) in &replacements {
        resolved_expr = resolved_expr.replace(placeholder, final_val);
    }
    select_parts.push(format!("{resolved_expr} AS {}", metric.name));

    let mut cte_sql = format!("SELECT {}", select_parts.join(", "));

    // FROM first input CTE.
    let _ = write!(
        cte_sql,
        " FROM {} AS {first_cte_alias}",
        first_effective_cte
    );

    // JOIN remaining input CTEs, accumulating aliases for COALESCE.
    let mut joined_aliases: Vec<String> = vec![first_cte_alias.clone()];
    for input in metric.input_metrics.iter().skip(1) {
        let alias = input.alias.as_deref().unwrap_or(&input.name);
        let cte_alias = format!("{alias}_cte");
        let eff_cte = effective_cte_name(input);

        let join_conditions: Vec<String> = join_cols
            .iter()
            .map(|col| {
                if joined_aliases.len() == 1 {
                    format!(
                        "{}.{col} IS NOT DISTINCT FROM {cte_alias}.{col}",
                        joined_aliases[0]
                    )
                } else {
                    let coalesce = joined_aliases
                        .iter()
                        .map(|a| format!("{a}.{col}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("COALESCE({coalesce}) IS NOT DISTINCT FROM {cte_alias}.{col}")
                }
            })
            .collect();

        if join_conditions.is_empty() {
            let _ = write!(cte_sql, " CROSS JOIN {} AS {cte_alias}", eff_cte);
        } else {
            let _ = write!(
                cte_sql,
                " FULL OUTER JOIN {} AS {cte_alias} ON {}",
                eff_cte,
                join_conditions.join(" AND ")
            );
        }
        joined_aliases.push(cte_alias);
    }

    // Replace group-by placeholders with COALESCE across all joined aliases.
    // Use join_cols (base CTE names) for column references, group_by_cols for output alias.
    let all_out_cols: Vec<&str> = group_by_cols
        .iter()
        .chain(where_only_dims.iter())
        .map(|s| s.as_str())
        .collect();
    for (jcol, out_col) in join_cols.iter().zip(all_out_cols.iter()) {
        let placeholder = format!("__GROUP_BY_PLACEHOLDER_{out_col}__");
        let replacement = if joined_aliases.len() == 1 {
            format!("{}.{jcol}", joined_aliases[0])
        } else {
            let parts: Vec<String> = joined_aliases
                .iter()
                .map(|a| format!("{a}.{jcol}"))
                .collect();
            format!("COALESCE({}) AS {out_col}", parts.join(", "))
        };
        cte_sql = cte_sql.replace(&placeholder, &replacement);
    }

    // Add custom granularity time spine JOIN if needed.
    if let Some((spine_rel, spine_col, time_name)) = &derived_cg_join {
        let _ = write!(
            cte_sql,
            " LEFT OUTER JOIN {spine_rel} AS ts_dcg ON {first_cte_alias}.{time_name} = ts_dcg.{spine_col}",
        );
    }

    // Apply deferred WHERE filters for offset metrics.
    // When offsets are present, WHERE filters are excluded from base CTEs and applied here
    // on the derived CTE output. Only dimension references that match group_by columns
    // can be resolved (they're carried through the CTE chain).
    if has_offsets && !spec.where_filters.is_empty() {
        let mut where_parts: Vec<String> = Vec::new();
        for filter in &spec.where_filters {
            // Resolve Dimension('entity__name') → first_cte_alias.entity__name
            let mut resolved = filter.clone();
            let mut cursor = 0usize;
            while let Some(start) = resolved[cursor..].find("Dimension('") {
                let abs_pos = cursor + start;
                if abs_pos > 0 && resolved.as_bytes()[abs_pos - 1].is_ascii_alphanumeric() {
                    cursor = abs_pos + 11;
                    continue;
                }
                let abs_start = abs_pos + 11;
                if let Some(end) = resolved[abs_start..].find("')") {
                    let dim_ref = resolved[abs_start..abs_start + end].to_string();
                    let col = group_by_cols
                        .iter()
                        .chain(where_only_dims.iter())
                        .find(|c| {
                            **c == dim_ref
                                || c.ends_with(&format!(
                                    "__{}",
                                    dim_ref.split("__").last().unwrap_or(&dim_ref)
                                ))
                        })
                        .cloned()
                        .unwrap_or(dim_ref);
                    let replacement = format!("{first_cte_alias}.{col}");
                    resolved.replace_range(abs_pos..abs_start + end + 2, &replacement);
                    cursor = abs_pos + replacement.len();
                } else {
                    break;
                }
            }
            // Also resolve TimeDimension('name', 'grain') → CTE column.
            let mut td_cursor = 0usize;
            while let Some(start) = resolved[td_cursor..].find("TimeDimension('") {
                let abs_pos = td_cursor + start;
                let abs_start = abs_pos + 15;
                if let Some(end) = resolved[abs_start..].find("')") {
                    let inner = &resolved[abs_start..abs_start + end];
                    let td_name = inner
                        .split(',')
                        .next()
                        .unwrap_or("metric_time")
                        .trim()
                        .trim_matches('\'');
                    let td_gran = inner
                        .split(',')
                        .nth(1)
                        .map(|s| s.trim().trim_matches('\'').trim_matches('"'))
                        .unwrap_or("day");

                    // Check for custom granularity that needs a time spine JOIN.
                    let cg_match = if !is_standard_granularity(td_gran) {
                        find_custom_granularity_spine(all_time_spines, td_gran)
                    } else {
                        None
                    };
                    if let Some((cg_spine, cg_col)) = cg_match {
                        let spine_rel = match dialect {
                            Dialect::Databricks => cg_spine.relation_name.replace('"', "`"),
                            _ => cg_spine.relation_name.clone(),
                        };
                        let spine_alias = "ts_cg_where";
                        let time_col = group_by_cols
                            .iter()
                            .find(|c| c.contains("metric_time") || c.contains(td_name))
                            .cloned()
                            .unwrap_or_else(|| "metric_time".to_string());
                        if !cte_sql.contains(spine_alias) {
                            let _ = write!(
                                cte_sql,
                                " LEFT OUTER JOIN {spine_rel} AS {spine_alias} \
                                 ON {first_cte_alias}.{time_col} = {spine_alias}.{}",
                                cg_spine.primary_column,
                            );
                        }
                        let replacement = format!("{spine_alias}.{cg_col}");
                        resolved.replace_range(abs_pos..abs_start + end + 2, &replacement);
                        td_cursor = abs_pos + replacement.len();
                    } else {
                        let col_name = format!("{td_name}__{td_gran}");
                        let col = group_by_cols
                            .iter()
                            .find(|c| {
                                **c == col_name
                                    || **c == td_name
                                    || c.ends_with(&format!("__{td_name}"))
                            })
                            .cloned()
                            .unwrap_or(col_name);
                        let replacement = col;
                        resolved.replace_range(abs_pos..abs_start + end + 2, &replacement);
                        td_cursor = abs_pos + replacement.len();
                    }
                } else {
                    break;
                }
            }
            // Strip remaining {{ }} Jinja wrappers.
            resolved = resolved.replace("{{ ", "").replace(" }}", "");
            where_parts.push(resolved);
        }
        if !where_parts.is_empty() {
            let _ = write!(cte_sql, " WHERE {}", where_parts.join(" AND "));
        }
    }

    // Skip if an offset wrapper CTE already produced a CTE with the same name
    // (happens when the derived metric is a simple passthrough of an offset alias).
    if !ctes.iter().any(|(name, _)| *name == scoped_name) {
        // When offsets forced day-level base CTEs but the spec wants coarser granularity,
        // wrap the derived CTE in an outer aggregation that truncates and re-aggregates.
        let offset_base_gran = base_spec
            .group_by
            .iter()
            .find_map(|g| {
                if let GroupBySpec::TimeDimension {
                    granularity: bg, ..
                } = g
                {
                    Some(bg.as_str())
                } else {
                    None
                }
            })
            .unwrap_or("day");
        let needs_reagg = has_offsets
            && spec.group_by.iter().any(|gb| {
                matches!(gb, GroupBySpec::TimeDimension { granularity, date_part, .. }
                     if granularity.as_str() != offset_base_gran || date_part.is_some())
            });
        let has_offset_to_grain = metric
            .input_metrics
            .iter()
            .any(|i| i.offset_to_grain.is_some());
        let has_custom_gran = spec.group_by.iter().any(|gb| {
            matches!(gb, GroupBySpec::TimeDimension { granularity, .. }
                if !is_standard_granularity(granularity))
        });
        if needs_reagg && (has_offset_to_grain || has_custom_gran) {
            // Re-aggregate each input metric individually to the query granularity,
            // then join them and compute the derived expression at the aggregated level.
            // This is necessary because SUM(a - b) != SUM(a) - SUM(b) when the FULL OUTER
            // JOIN produces NULLs on one side.

            // Build truncation expressions for time dimensions at query granularity.
            // Source CTEs have columns named by join_cols (base granularity),
            // output needs columns named by group_by_cols (query granularity).
            let mut trunc_select: Vec<String> = Vec::new();
            let mut trunc_group: Vec<String> = Vec::new();
            let mut out_cols_reagg: Vec<String> = Vec::new();
            let mut reagg_idx = 1;
            // Map from spec.group_by to the source column name (join_col).
            // Time dimensions share a base name (e.g., "metric_time") in join_cols.
            let mut jcol_iter = join_cols.iter();
            let mut seen_time_names: HashSet<String> = HashSet::new();
            for (gb, out_col) in spec.group_by.iter().zip(group_by_cols.iter()) {
                let src_col = match gb {
                    GroupBySpec::TimeDimension { name, .. } => {
                        if seen_time_names.insert(name.clone()) {
                            jcol_iter
                                .next()
                                .map(|s| s.as_str())
                                .unwrap_or(out_col.as_str())
                        } else {
                            name.as_str()
                        }
                    }
                    _ => jcol_iter
                        .next()
                        .map(|s| s.as_str())
                        .unwrap_or(out_col.as_str()),
                };
                match gb {
                    GroupBySpec::TimeDimension {
                        granularity,
                        date_part,
                        ..
                    } if granularity.as_str() != offset_base_gran || date_part.is_some() => {
                        if date_part.is_some() {
                            trunc_select.push(format!("{src_col} AS {out_col}"));
                        } else {
                            let trunc = render_date_trunc(granularity, src_col, dialect);
                            trunc_select.push(format!("{trunc} AS {out_col}"));
                        }
                        trunc_group.push(reagg_idx.to_string());
                        out_cols_reagg.push(out_col.clone());
                        reagg_idx += 1;
                    }
                    _ => {
                        if src_col != out_col {
                            trunc_select.push(format!("{src_col} AS {out_col}"));
                        } else {
                            trunc_select.push(out_col.clone());
                        }
                        trunc_group.push(reagg_idx.to_string());
                        out_cols_reagg.push(out_col.clone());
                        reagg_idx += 1;
                    }
                }
            }

            // Create a re-aggregation CTE for each input metric.
            let mut reagg_cte_names: Vec<(String, String)> = Vec::new(); // (alias, reagg_cte_name)
            for input in &metric.input_metrics {
                let alias = input.alias.as_deref().unwrap_or(&input.name);
                let eff_cte = effective_cte_name(input);
                let reagg_name = format!("{eff_cte}_reagg");

                if let Some(ref grain) = input.offset_to_grain {
                    // For offset-to-grain inputs at non-default granularity, redo the
                    // spine join considering only granularity-boundary dates.
                    // Reference approach: filter spine to dates matching the granularity
                    // start (e.g., Mondays for week), then join to base and group.
                    let spine_cte = format!("{}_spine", alias);
                    let base_cte = input.name.clone();
                    let time_col = join_cols
                        .first()
                        .map(|s| s.as_str())
                        .unwrap_or("metric_time");
                    let mut sel: Vec<String> = Vec::new();
                    let mut grp: Vec<String> = Vec::new();
                    // Collect all query granularities for filtering.
                    let mut query_grans: Vec<&str> = Vec::new();
                    for (gi, (gb, out_col)) in
                        (1..).zip(spec.group_by.iter().zip(group_by_cols.iter()))
                    {
                        if let GroupBySpec::TimeDimension { granularity, .. } = gb {
                            let trunc = render_date_trunc(
                                granularity,
                                &format!("spine.{time_col}"),
                                dialect,
                            );
                            sel.push(format!("{trunc} AS {out_col}"));
                            query_grans.push(granularity.as_str());
                        } else {
                            sel.push(out_col.clone());
                        }
                        grp.push(gi.to_string());
                    }
                    sel.push(format!("SUM(base.{}) AS {}", input.name, input.name));
                    // Filter spine to only granularity-boundary dates (start of week, month, etc.)
                    // using OR across all query granularities.
                    let spine_filters: Vec<String> = query_grans
                        .iter()
                        .map(|g| {
                            let trunc = render_date_trunc(g, &format!("spine.{time_col}"), dialect);
                            format!("{trunc} = spine.{time_col}")
                        })
                        .collect();
                    let where_clause = if spine_filters.len() == 1 {
                        spine_filters[0].clone()
                    } else {
                        spine_filters.join(" OR ")
                    };
                    let sql = format!(
                        "SELECT {} FROM {spine_cte} AS spine INNER JOIN {base_cte} AS base ON DATE_TRUNC('{grain}', spine.{time_col}) = base.{time_col} WHERE {where_clause} GROUP BY {}",
                        sel.join(", "),
                        grp.join(", "),
                    );
                    ctes.push((reagg_name.clone(), sql));
                } else if let Some(offset_window) = &input.offset_window {
                    let base_cte = input.name.clone();
                    let time_col = join_cols
                        .first()
                        .map(|s| s.as_str())
                        .unwrap_or("metric_time");
                    let interval = render_interval_str(offset_window, dialect);
                    let mut sel: Vec<String> = Vec::new();
                    let mut grp: Vec<String> = Vec::new();
                    let mut cg_join = String::new();
                    for (gi, (gb, out_col)) in
                        (1..).zip(spec.group_by.iter().zip(group_by_cols.iter()))
                    {
                        if let GroupBySpec::TimeDimension { granularity, .. } = gb {
                            if !is_standard_granularity(granularity) {
                                if let Some((spine, cg_col)) =
                                    find_custom_granularity_spine(all_time_spines, granularity)
                                {
                                    sel.push(format!("ts_reagg.{cg_col} AS {out_col}"));
                                    if cg_join.is_empty() {
                                        cg_join = format!(
                                            " LEFT OUTER JOIN {} ts_reagg ON spine.{time_col} = ts_reagg.{}",
                                            spine.relation_name, spine.primary_column,
                                        );
                                    }
                                } else {
                                    sel.push(format!("spine.{time_col} AS {out_col}"));
                                }
                            } else {
                                let trunc = render_date_trunc(
                                    granularity,
                                    &format!("spine.{time_col}"),
                                    dialect,
                                );
                                sel.push(format!("{trunc} AS {out_col}"));
                            }
                        } else {
                            sel.push(out_col.clone());
                        }
                        grp.push(gi.to_string());
                    }
                    sel.push(format!("SUM(base.{}) AS {}", input.name, input.name));
                    let spine_cte = format!("{eff_cte}_spine");
                    let sql = format!(
                        "SELECT {} FROM {spine_cte} AS spine INNER JOIN {base_cte} AS base ON spine.{time_col} - {interval} = base.{time_col}{cg_join} GROUP BY {}",
                        sel.join(", "),
                        grp.join(", "),
                    );
                    ctes.push((reagg_name.clone(), sql));
                } else if has_custom_gran {
                    // For non-offset inputs with custom granularity, recompile
                    // the metric at the custom granularity level to preserve
                    // non-additive aggregation semantics (e.g., COUNT_DISTINCT).
                    let mut recompiled = false;
                    if let Some(input_metric) = all_metrics.get(&input.name) {
                        if input_metric.metric_type == MetricType::Simple {
                            let cg_spec = SemanticQuerySpec {
                                group_by: spec.group_by.clone(),
                                ..base_spec.clone()
                            };
                            let cte_count_before = ctes.len();
                            compile_simple_metric_cte(
                                input_metric,
                                &cg_spec,
                                all_metrics,
                                model_aliases,
                                join_edges,
                                all_time_spines,
                                dialect,
                                "cg_reagg",
                                ctes,
                            )?;
                            // Rename the last CTE to the expected reagg name.
                            if ctes.len() > cte_count_before {
                                let last = ctes.last_mut().unwrap();
                                last.0 = reagg_name.clone();
                            }
                            recompiled = true;
                        }
                    }
                    if !recompiled {
                        let time_col = join_cols
                            .first()
                            .map(|s| s.as_str())
                            .unwrap_or("metric_time");
                        let mut sel: Vec<String> = Vec::new();
                        let mut grp: Vec<String> = Vec::new();
                        let mut cg_join = String::new();
                        for (gi, (gb, out_col)) in
                            (1usize..).zip(spec.group_by.iter().zip(group_by_cols.iter()))
                        {
                            if let GroupBySpec::TimeDimension { granularity, .. } = gb {
                                if !is_standard_granularity(granularity) {
                                    if let Some((spine, cg_col)) =
                                        find_custom_granularity_spine(all_time_spines, granularity)
                                    {
                                        sel.push(format!("ts_reagg.{cg_col} AS {out_col}"));
                                        if cg_join.is_empty() {
                                            cg_join = format!(
                                                " LEFT OUTER JOIN {} ts_reagg ON base.{time_col} = ts_reagg.{}",
                                                spine.relation_name, spine.primary_column,
                                            );
                                        }
                                    } else {
                                        sel.push(format!("base.{time_col} AS {out_col}"));
                                    }
                                } else {
                                    let trunc = render_date_trunc(
                                        granularity,
                                        &format!("base.{time_col}"),
                                        dialect,
                                    );
                                    sel.push(format!("{trunc} AS {out_col}"));
                                }
                            } else {
                                sel.push(format!("base.{out_col}"));
                            }
                            grp.push(gi.to_string());
                        }
                        sel.push(format!("SUM(base.{}) AS {}", input.name, input.name));
                        let sql = format!(
                            "SELECT {} FROM {eff_cte} AS base{cg_join} GROUP BY {}",
                            sel.join(", "),
                            grp.join(", "),
                        );
                        ctes.push((reagg_name.clone(), sql));
                    }
                } else {
                    // For non-offset inputs, simple truncation and re-aggregation.
                    let mut sel = trunc_select.clone();
                    sel.push(format!("SUM({}) AS {}", input.name, input.name));
                    let sql = format!(
                        "SELECT {} FROM {eff_cte} GROUP BY {}",
                        sel.join(", "),
                        trunc_group.join(", "),
                    );
                    ctes.push((reagg_name.clone(), sql));
                }
                reagg_cte_names.push((alias.to_string(), reagg_name));
            }

            // Build the derived expression joining re-aggregated CTEs.
            let first_alias = format!("{}_cte", reagg_cte_names[0].0);
            let mut reagg_select: Vec<String> = Vec::new();
            let mut reagg_joined: Vec<String> = vec![first_alias.clone()];
            for col in &out_cols_reagg {
                reagg_select.push(format!("__REAGG_PH_{col}__"));
            }

            // Build expression referencing re-aggregated CTEs.
            let mut reagg_expr = expr.to_string();
            let mut reps: Vec<(String, String, String)> = reagg_cte_names
                .iter()
                .enumerate()
                .map(|(i, (alias, _reagg_cte))| {
                    let cte_alias = format!("{alias}_cte");
                    let placeholder = format!("\x00RA{i}\x00");
                    let col_ref = format!("{cte_alias}.{}", metric.input_metrics[i].name);
                    let final_ref = if let Some(fill) = all_metrics
                        .get(&metric.input_metrics[i].name)
                        .and_then(|m| m.fill_nulls_with)
                    {
                        format!("COALESCE({col_ref}, {fill})")
                    } else {
                        col_ref
                    };
                    (alias.clone(), placeholder, final_ref)
                })
                .collect();
            reps.sort_by_key(|a| std::cmp::Reverse(a.0.len()));
            for (find, placeholder, _) in &reps {
                reagg_expr = replace_word(&reagg_expr, find, placeholder);
            }
            for (_, placeholder, final_val) in &reps {
                reagg_expr = reagg_expr.replace(placeholder, final_val);
            }
            reagg_select.push(format!("{reagg_expr} AS {}", metric.name));

            let mut reagg_sql = format!(
                "SELECT {} FROM {} AS {first_alias}",
                reagg_select.join(", "),
                reagg_cte_names[0].1
            );
            for (alias, reagg_cte) in reagg_cte_names.iter().skip(1) {
                let cte_alias = format!("{alias}_cte");
                let join_conds: Vec<String> = out_cols_reagg
                    .iter()
                    .map(|col| {
                        if reagg_joined.len() == 1 {
                            format!(
                                "{}.{col} IS NOT DISTINCT FROM {cte_alias}.{col}",
                                reagg_joined[0]
                            )
                        } else {
                            let coalesce = reagg_joined
                                .iter()
                                .map(|a| format!("{a}.{col}"))
                                .collect::<Vec<_>>()
                                .join(", ");
                            format!("COALESCE({coalesce}) IS NOT DISTINCT FROM {cte_alias}.{col}")
                        }
                    })
                    .collect();
                use std::fmt::Write as _;
                let _ = write!(
                    reagg_sql,
                    " FULL OUTER JOIN {} AS {cte_alias} ON {}",
                    reagg_cte,
                    join_conds.join(" AND ")
                );
                reagg_joined.push(cte_alias);
            }

            // Replace group-by placeholders with COALESCE.
            for col in &out_cols_reagg {
                let placeholder = format!("__REAGG_PH_{col}__");
                let replacement = if reagg_joined.len() == 1 {
                    format!("{}.{col}", reagg_joined[0])
                } else {
                    let parts: Vec<String> =
                        reagg_joined.iter().map(|a| format!("{a}.{col}")).collect();
                    format!("COALESCE({}) AS {col}", parts.join(", "))
                };
                reagg_sql = reagg_sql.replace(&placeholder, &replacement);
            }

            ctes.push((scoped_name, reagg_sql));
        } else if needs_reagg {
            if derived_cg_join.is_some() {
                // Custom granularity: the derived CTE already has the custom gran column
                // via time spine JOIN, but we still need to aggregate from day-level rows.
                // The output column is already named metric_time__alien_day (via ts_dcg.*).
                // Just add GROUP BY for the custom gran column(s) + SUM the metric.
                let mut cg_select: Vec<String> = Vec::new();
                let mut cg_group: Vec<String> = Vec::new();
                for (cg_idx, out_col) in (1..).zip(group_by_cols.iter()) {
                    cg_select.push(out_col.clone());
                    cg_group.push(cg_idx.to_string());
                }
                cg_select.push(format!("SUM({}) AS {}", metric.name, metric.name));
                let inner_name = format!("{scoped_name}_ungrouped");
                ctes.push((inner_name.clone(), cte_sql));
                let cg_sql = format!(
                    "SELECT {} FROM {inner_name} GROUP BY {}",
                    cg_select.join(", "),
                    cg_group.join(", "),
                );
                ctes.push((scoped_name, cg_sql));
            } else {
                let inner_name = format!("{scoped_name}_{offset_base_gran}");
                ctes.push((inner_name.clone(), cte_sql));
                let mut outer_select: Vec<String> = Vec::new();
                let mut outer_group: Vec<String> = Vec::new();
                let mut idx = 1;
                for (gb, out_col) in spec.group_by.iter().zip(group_by_cols.iter()) {
                    match gb {
                        GroupBySpec::TimeDimension {
                            granularity,
                            date_part,
                            ..
                        } if granularity.as_str() != offset_base_gran || date_part.is_some() => {
                            if date_part.is_some() {
                                outer_select.push(out_col.clone());
                            } else {
                                let trunc = render_date_trunc(granularity, out_col, dialect);
                                outer_select.push(format!("{trunc} AS {out_col}"));
                            }
                            outer_group.push(idx.to_string());
                            idx += 1;
                        }
                        _ => {
                            outer_select.push(out_col.clone());
                            outer_group.push(idx.to_string());
                            idx += 1;
                        }
                    }
                }
                outer_select.push(format!("SUM({}) AS {}", metric.name, metric.name));
                let outer_sql = format!(
                    "SELECT {} FROM {inner_name} GROUP BY {}",
                    outer_select.join(", "),
                    outer_group.join(", "),
                );
                ctes.push((scoped_name, outer_sql));
            }
        } else if !where_only_dims.is_empty() {
            // Extra WHERE-only dims are in the CTE output; strip them via a wrapper.
            let inner_name = format!("{scoped_name}_filtered");
            ctes.push((inner_name.clone(), cte_sql));
            let wanted: Vec<&str> = group_by_cols
                .iter()
                .map(|s| s.as_str())
                .chain(std::iter::once(metric.name.as_str()))
                .collect();
            ctes.push((
                scoped_name,
                format!("SELECT {} FROM {inner_name}", wanted.join(", ")),
            ));
        } else {
            ctes.push((scoped_name, cte_sql));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn compile_ratio_metric_cte(
    metric: &ResolvedMetric,
    spec: &SemanticQuerySpec,
    all_metrics: &HashMap<String, ResolvedMetric>,
    model_aliases: &HashMap<String, (String, &ResolvedModel)>,
    join_edges: &[JoinEdge],
    all_time_spines: &[TimeSpine],
    dialect: Dialect,
    cte_scope: &str,
    ctes: &mut Vec<(String, String)>,
) -> Result<(), MetricFlowError> {
    let numerator = metric.numerator.as_ref().ok_or_else(|| {
        MetricFlowError::Other(format!("ratio metric {} has no numerator", metric.name))
    })?;
    let denominator = metric.denominator.as_ref().ok_or_else(|| {
        MetricFlowError::Other(format!("ratio metric {} has no denominator", metric.name))
    })?;

    // Propagate the ratio metric's own metric_filters to both inputs.
    let base_spec;
    let base_scope;
    let (base_spec_ref, base_scope_ref) = if metric.metric_filters.is_empty() {
        (spec, cte_scope)
    } else {
        base_spec = SemanticQuerySpec {
            where_filters: spec
                .where_filters
                .iter()
                .cloned()
                .chain(metric.metric_filters.iter().cloned())
                .collect(),
            ..spec.clone()
        };
        base_scope = metric.name.clone();
        (&base_spec, base_scope.as_str())
    };

    // Compile numerator and denominator as CTEs.
    // If a numerator/denominator has per-input filters, propagate them via the spec
    // and use a scoped CTE name to avoid collisions.
    let num_spec;
    let num_scope;
    let (num_spec_ref, num_scope_ref) = if numerator.filters.is_empty() {
        (base_spec_ref, base_scope_ref)
    } else {
        num_spec = SemanticQuerySpec {
            where_filters: base_spec_ref
                .where_filters
                .iter()
                .cloned()
                .chain(numerator.filters.iter().cloned())
                .collect(),
            ..base_spec_ref.clone()
        };
        num_scope = format!("{}_num", metric.name);
        (&num_spec, num_scope.as_str())
    };
    if let Some(num_metric) = all_metrics.get(&numerator.name) {
        match num_metric.metric_type {
            MetricType::Simple => {
                compile_simple_metric_cte(
                    num_metric,
                    num_spec_ref,
                    all_metrics,
                    model_aliases,
                    join_edges,
                    all_time_spines,
                    dialect,
                    num_scope_ref,
                    ctes,
                )?;
            }
            MetricType::Derived => {
                compile_derived_metric_cte(
                    num_metric,
                    num_spec_ref,
                    all_metrics,
                    model_aliases,
                    join_edges,
                    dialect,
                    num_scope_ref,
                    ctes,
                    all_time_spines,
                )?;
            }
            _ => {}
        }
    }
    let den_spec;
    let den_scope;
    let (den_spec_ref, den_scope_ref) = if denominator.filters.is_empty() {
        (base_spec_ref, base_scope_ref)
    } else {
        den_spec = SemanticQuerySpec {
            where_filters: base_spec_ref
                .where_filters
                .iter()
                .cloned()
                .chain(denominator.filters.iter().cloned())
                .collect(),
            ..base_spec_ref.clone()
        };
        den_scope = format!("{}_den", metric.name);
        (&den_spec, den_scope.as_str())
    };
    if let Some(den_metric) = all_metrics.get(&denominator.name) {
        match den_metric.metric_type {
            MetricType::Simple => {
                compile_simple_metric_cte(
                    den_metric,
                    den_spec_ref,
                    all_metrics,
                    model_aliases,
                    join_edges,
                    all_time_spines,
                    dialect,
                    den_scope_ref,
                    ctes,
                )?;
            }
            MetricType::Derived => {
                compile_derived_metric_cte(
                    den_metric,
                    den_spec_ref,
                    all_metrics,
                    model_aliases,
                    join_edges,
                    dialect,
                    den_scope_ref,
                    ctes,
                    all_time_spines,
                )?;
            }
            _ => {}
        }
    }

    // Build the ratio CTE: reference the scoped CTE names for numerator and denominator.
    let num_cte_name = if numerator.filters.is_empty() {
        if base_scope_ref.is_empty() {
            numerator.name.clone()
        } else {
            format!("{base_scope_ref}__{}", numerator.name)
        }
    } else {
        format!("{}_num__{}", metric.name, numerator.name)
    };
    let den_cte_name = if denominator.filters.is_empty() {
        if base_scope_ref.is_empty() {
            denominator.name.clone()
        } else {
            format!("{base_scope_ref}__{}", denominator.name)
        }
    } else {
        format!("{}_den__{}", metric.name, denominator.name)
    };
    let group_by_cols = group_by_output_cols(&spec.group_by);

    let num_alias = "num";
    let den_alias = "den";

    let mut select_parts: Vec<String> = Vec::new();
    for col in &group_by_cols {
        select_parts.push(format!(
            "COALESCE({num_alias}.{col}, {den_alias}.{col}) AS {col}"
        ));
    }

    let cast_num = render_cast_double(&format!("{num_alias}.{}", numerator.name), dialect);
    let cast_den = render_cast_double(&format!("{den_alias}.{}", denominator.name), dialect);
    select_parts.push(format!(
        "{cast_num} / NULLIF({cast_den}, 0) AS {}",
        metric.name
    ));

    let mut cte_sql = format!("SELECT {}", select_parts.join(", "));
    let _ = write!(cte_sql, " FROM {num_cte_name} AS {num_alias}");

    let join_conditions: Vec<String> = group_by_cols
        .iter()
        .map(|col| format!("{num_alias}.{col} IS NOT DISTINCT FROM {den_alias}.{col}"))
        .collect();

    if join_conditions.is_empty() {
        let _ = write!(cte_sql, " CROSS JOIN {den_cte_name} AS {den_alias}");
    } else {
        let _ = write!(
            cte_sql,
            " FULL OUTER JOIN {den_cte_name} AS {den_alias} ON {}",
            join_conditions.join(" AND ")
        );
    }

    ctes.push((metric.name.clone(), cte_sql));
    Ok(())
}

#[allow(clippy::too_many_arguments, clippy::cognitive_complexity)]
fn compile_cumulative_metric_cte(
    metric: &ResolvedMetric,
    spec: &SemanticQuerySpec,
    all_metrics: &HashMap<String, ResolvedMetric>,
    model_aliases: &HashMap<String, (String, &ResolvedModel)>,
    join_edges: &[JoinEdge],
    dialect: Dialect,
    _time_spine: Option<&TimeSpine>,
    ctes: &mut Vec<(String, String)>,
    all_time_spines: &[TimeSpine],
) -> Result<(), MetricFlowError> {
    // Cumulative metrics use an inline time spine (generate_series) joined to the
    // source data.  Three patterns:
    //   - All-time:       src.time <= spine.time
    //   - Rolling window: src.time <= spine.time AND src.time > spine.time - interval
    //   - Grain-to-date:  src.time <= spine.time AND src.time >= date_trunc(grain, spine.time)

    if ctes.iter().any(|(name, _)| name == &metric.name) {
        return Ok(());
    }

    let cp = metric.cumulative_params.as_ref().ok_or_else(|| {
        MetricFlowError::Other(format!(
            "cumulative metric {} has no cumulative params",
            metric.name
        ))
    })?;

    // Resolve aggregation params — either directly on this metric or via an input metric.
    let ap = metric
        .agg_params
        .as_ref()
        .or_else(|| {
            metric
                .input_metrics
                .first()
                .and_then(|inp| all_metrics.get(&inp.name))
                .and_then(|m| m.agg_params.as_ref())
        })
        .ok_or_else(|| {
            MetricFlowError::Other(format!(
                "cumulative metric {} has no aggregation params",
                metric.name
            ))
        })?;

    let (primary_alias, primary_model) =
        model_aliases.get(&ap.semantic_model).ok_or_else(|| {
            MetricFlowError::Other(format!(
                "semantic model not resolved: {}",
                ap.semantic_model
            ))
        })?;

    let from_relation = render_full_relation(primary_model, dialect);

    // Resolve the raw time column on the source table (e.g., "created_at" or "ds").
    let time_col_raw = ap
        .agg_time_dimension
        .as_deref()
        .map(|dim_name| {
            primary_model
                .dimensions
                .iter()
                .find(|d| d.name == dim_name)
                .map(|d| d.expr.as_str())
                .unwrap_or(dim_name)
        })
        .unwrap_or("ds");

    let has_time_dim = spec
        .group_by
        .iter()
        .any(|gb| matches!(gb, GroupBySpec::TimeDimension { .. }));

    let granularity = spec
        .group_by
        .iter()
        .find_map(|gb| {
            if let GroupBySpec::TimeDimension { granularity, .. } = gb {
                Some(granularity.as_str())
            } else {
                None
            }
        })
        .unwrap_or("day");

    // Measure expression in the source CTE.
    let measure_col = qualify_measure_expr("f", &ap.expr);
    let agg_src = render_agg_with_params(
        &ap.agg,
        "src.measure_value",
        dialect,
        ap.percentile,
        ap.use_discrete_percentile,
    );

    // Collect non-time dimension output names for GROUP BY pass-through.
    let dim_col_names: Vec<String> = spec
        .group_by
        .iter()
        .filter_map(|gb| match gb {
            GroupBySpec::Dimension {
                entity: Some(e),
                name,
            } => Some(format!("{e}__{name}")),
            GroupBySpec::Dimension { entity: None, name } => Some(name.clone()),
            GroupBySpec::Entity { name } => Some(name.clone()),
            _ => None,
        })
        .collect();

    if !has_time_dim {
        // No time dimension — falls back to a simple aggregate (no spine needed).
        let col_expr = qualify_measure_expr(primary_alias, &ap.expr);
        let agg_expr = render_agg_with_params(
            &ap.agg,
            &col_expr,
            dialect,
            ap.percentile,
            ap.use_discrete_percentile,
        );
        let mut select_parts: Vec<String> = Vec::new();
        // Add dimension group-by columns.
        for gb in &spec.group_by {
            match gb {
                GroupBySpec::Dimension { entity, name } => {
                    let dim_ref = match entity {
                        Some(e) => format!("{e}__{name}"),
                        None => name.clone(),
                    };
                    let scoped = scoped_aliases_for(ap, primary_alias, primary_model);
                    let col = resolve_dimension_ref(&dim_ref, &scoped, dialect, "");
                    let output = match entity {
                        Some(e) => format!("{e}__{name}"),
                        None => name.clone(),
                    };
                    select_parts.push(format!("{col} AS {output}"));
                }
                GroupBySpec::Entity { name } => {
                    let scoped = scoped_aliases_for(ap, primary_alias, primary_model);
                    let col = resolve_entity_ref(name, &scoped, "");
                    select_parts.push(format!("{col} AS {name}"));
                }
                _ => {}
            }
        }
        select_parts.push(format!("{agg_expr} AS {}", metric.name));
        let mut cte_sql = format!(
            "SELECT {} FROM {from_relation} AS {primary_alias}",
            select_parts.join(", ")
        );
        add_dimension_joins(
            spec,
            &metric.metric_filters,
            &ap.semantic_model,
            primary_alias,
            model_aliases,
            join_edges,
            dialect,
            &mut cte_sql,
            &[],
            None,
        );
        {
            let mut where_parts: Vec<String> = Vec::new();
            for filter in &metric.metric_filters {
                where_parts.push(resolve_where_filter(
                    filter,
                    model_aliases,
                    dialect,
                    &ap.semantic_model,
                ));
            }
            for filter in &spec.where_filters {
                where_parts.push(resolve_where_filter(
                    filter,
                    model_aliases,
                    dialect,
                    &ap.semantic_model,
                ));
            }
            if !where_parts.is_empty() {
                let _ = write!(cte_sql, " WHERE {}", where_parts.join(" AND "));
            }
        }
        if !spec.group_by.is_empty() {
            let indices: Vec<String> = (1..=spec.group_by.len()).map(|i| i.to_string()).collect();
            let _ = write!(cte_sql, " GROUP BY {}", indices.join(", "));
        }
        ctes.push((metric.name.clone(), cte_sql));
        return Ok(());
    }

    // ── Step 1: Source data CTE ─────────────────────────────────────────
    let src_cte = format!("{}_src", metric.name);
    let mut src_select = vec![format!("f.{time_col_raw} AS src_time")];
    src_select.push(format!("{measure_col} AS measure_value"));
    // Include dimension columns from joins for pass-through.
    {
        let scoped = scoped_aliases_for(ap, primary_alias, primary_model);
        for gb in &spec.group_by {
            match gb {
                GroupBySpec::Dimension { entity, name } => {
                    let dim_ref = match entity {
                        Some(e) => format!("{e}__{name}"),
                        None => name.clone(),
                    };
                    let col = resolve_dimension_ref(&dim_ref, &scoped, dialect, "");
                    let output = match entity {
                        Some(e) => format!("{e}__{name}"),
                        None => name.clone(),
                    };
                    src_select.push(format!("{col} AS {output}"));
                }
                GroupBySpec::Entity { name } => {
                    let col = resolve_entity_ref(name, &scoped, "");
                    src_select.push(format!("{col} AS {name}"));
                }
                _ => {}
            }
        }
    }
    let mut src_sql = format!("SELECT {} FROM {from_relation} AS f", src_select.join(", "));
    // Use "f" as the primary alias for joins in the source CTE.
    add_dimension_joins(
        spec,
        &metric.metric_filters,
        &ap.semantic_model,
        "f",
        model_aliases,
        join_edges,
        dialect,
        &mut src_sql,
        &[],
        None,
    );
    // For cumulative metrics, time-dimension filters must be applied AFTER the
    // cumulative aggregation (on the spine output), not on the source data,
    // because the source needs historical data for the cumulative window.
    // Separate filters into time-referencing (deferred) and non-time (applied to source).
    let time_dim_output_names: Vec<String> = spec
        .group_by
        .iter()
        .filter_map(|gb| {
            if let GroupBySpec::TimeDimension {
                name, granularity, ..
            } = gb
            {
                Some(format!("{name}__{granularity}"))
            } else {
                None
            }
        })
        .collect();
    let is_time_filter = |f: &str| -> bool {
        time_dim_output_names
            .iter()
            .any(|td| f.contains(td.as_str()))
            || f.contains("metric_time")
            || f.contains("TimeDimension(")
    };
    let mut deferred_where_parts: Vec<String> = Vec::new();
    {
        let mut where_parts: Vec<String> = Vec::new();
        for filter in &metric.metric_filters {
            if is_time_filter(filter) {
                deferred_where_parts.push(filter.clone());
            } else {
                where_parts.push(resolve_where_filter(
                    filter,
                    model_aliases,
                    dialect,
                    &ap.semantic_model,
                ));
            }
        }
        for filter in &spec.where_filters {
            if is_time_filter(filter) {
                deferred_where_parts.push(filter.clone());
            } else {
                where_parts.push(resolve_where_filter(
                    filter,
                    model_aliases,
                    dialect,
                    &ap.semantic_model,
                ));
            }
        }
        if !where_parts.is_empty() {
            let _ = write!(src_sql, " WHERE {}", where_parts.join(" AND "));
        }
    }
    ctes.push((src_cte.clone(), src_sql));

    // ── Step 2: Inline time spine ─────────────────────────────────────────
    // Cumulative metrics always use a daily spine for accurate rolling windows;
    // the query granularity (week/month) is applied via DATE_TRUNC in the output.
    let spine_gran = if matches!(granularity, "hour" | "minute" | "second") {
        granularity
    } else {
        "day"
    };
    let spine_cte = format!("{}_spine", metric.name);
    let tc_start = spec
        .time_constraint
        .as_ref()
        .map(|(start, _)| start.as_str());
    let tc_end = spec.time_constraint.as_ref().map(|(_, end)| end.as_str());
    let window_extension =
        if let (Some(count), Some(gran)) = (&cp.window_count, &cp.window_granularity) {
            Some(render_interval(*count, gran, dialect))
        } else {
            None
        };
    let is_all_time = cp.window_count.is_none() && cp.grain_to_date.is_none();
    let spine_sql = if is_all_time {
        if let Some(ts) = pick_time_spine_for_granularity(all_time_spines, spine_gran) {
            let cast = render_type_cast(
                &format!("t.{}", ts.primary_column),
                if matches!(spine_gran, "hour" | "minute" | "second") {
                    "TIMESTAMP"
                } else {
                    "DATE"
                },
                dialect,
            );
            let spine_rel = match dialect {
                Dialect::Databricks => ts.relation_name.replace('"', "`"),
                _ => ts.relation_name.clone(),
            };
            format!("SELECT {cast} AS spine_time FROM {spine_rel} AS t")
        } else {
            inline_time_spine_sql_bounded(
                "spine_time",
                &src_cte,
                "src_time",
                spine_gran,
                dialect,
                tc_end,
            )
        }
    } else if let Some(ref ext) = window_extension {
        let subdaily = matches!(spine_gran, "hour" | "minute" | "second");
        let target_type = if subdaily { "TIMESTAMP" } else { "DATE" };
        let min_expr = if let Some(start) = tc_start {
            format!("GREATEST((SELECT MIN(src_time) FROM {src_cte}), CAST('{start}' AS DATE))")
        } else {
            format!("(SELECT MIN(src_time) FROM {src_cte})")
        };
        let max_expr = if let Some(bound) = tc_end {
            format!(
                "GREATEST((SELECT MAX(src_time) FROM {src_cte}) + {ext}, CAST('{bound}' AS DATE))"
            )
        } else {
            format!("(SELECT MAX(src_time) FROM {src_cte}) + {ext}")
        };
        match dialect {
            Dialect::DuckDB => {
                let cast = render_type_cast("ds", target_type, dialect);
                format!(
                    "SELECT {cast} AS spine_time FROM generate_series({min_expr}, {max_expr}, INTERVAL '1 {spine_gran}') AS t(ds)"
                )
            }
            _ => inline_time_spine_sql_bounded(
                "spine_time",
                &src_cte,
                "src_time",
                spine_gran,
                dialect,
                tc_end,
            ),
        }
    } else {
        inline_time_spine_sql_bounded(
            "spine_time",
            &src_cte,
            "src_time",
            spine_gran,
            dialect,
            tc_end,
        )
    };
    ctes.push((spine_cte.clone(), spine_sql));

    // ── Step 3: Join spine → source with time range predicate ────────────
    let join_cond = if let (Some(count), Some(gran)) = (&cp.window_count, &cp.window_granularity) {
        let interval = render_interval(*count, gran, dialect);
        format!(
            "src.src_time <= spine.spine_time \
             AND src.src_time > spine.spine_time - {interval}"
        )
    } else if let Some(ref grain) = cp.grain_to_date {
        format!(
            "src.src_time <= spine.spine_time \
             AND src.src_time >= DATE_TRUNC('{grain}', spine.spine_time)"
        )
    } else {
        "src.src_time <= spine.spine_time".to_string()
    };

    let time_out_col = spec
        .group_by
        .iter()
        .find_map(|gb| {
            if let GroupBySpec::TimeDimension { name, .. } = gb {
                Some(name.as_str())
            } else {
                None
            }
        })
        .unwrap_or("metric_time");

    // Collect all time dimension specs for non-default-grain handling.
    let time_dims: Vec<(&str, &str)> = spec
        .group_by
        .iter()
        .filter_map(|gb| {
            if let GroupBySpec::TimeDimension {
                name, granularity, ..
            } = gb
            {
                Some((name.as_str(), granularity.as_str()))
            } else {
                None
            }
        })
        .collect();
    let has_non_default_grain = time_dims.len() > 1
        || time_dims
            .iter()
            .any(|(_, g)| !matches!(*g, "day" | "hour" | "minute" | "second"));

    let mut cum_select: Vec<String>;
    let mut cum_group: Vec<String>;

    let mut cum_extra_join = String::new();
    if has_non_default_grain {
        // Daily spine with all coarser-grain truncations as extra columns.
        cum_select = vec![format!("spine.spine_time AS {time_out_col}__day")];
        cum_group = vec!["1".to_string()];
        let mut idx = 2;
        let mut cg_counter = 0u32;
        for (td_name, td_gran) in &time_dims {
            let col_name = format!("{td_name}__{td_gran}");
            if !is_standard_granularity(td_gran) {
                if let Some((cg_spine, cg_col)) =
                    find_custom_granularity_spine(all_time_spines, td_gran)
                {
                    let cg_alias = if cg_counter == 0 {
                        "ts_cg".to_string()
                    } else {
                        format!("ts_cg{cg_counter}")
                    };
                    cg_counter += 1;
                    let spine_rel = match dialect {
                        Dialect::Databricks => cg_spine.relation_name.replace('"', "`"),
                        _ => cg_spine.relation_name.clone(),
                    };
                    cum_extra_join.push_str(&format!(
                        " LEFT OUTER JOIN {spine_rel} AS {cg_alias} ON spine.spine_time = {cg_alias}.{}",
                        cg_spine.primary_column,
                    ));
                    cum_select.push(format!("{cg_alias}.{cg_col} AS {col_name}"));
                } else {
                    cum_select.push(format!(
                        "DATE_TRUNC('{td_gran}', spine.spine_time) AS {col_name}"
                    ));
                }
            } else {
                cum_select.push(format!(
                    "DATE_TRUNC('{td_gran}', spine.spine_time) AS {col_name}"
                ));
            }
            cum_group.push(idx.to_string());
            idx += 1;
        }
        for dcn in &dim_col_names {
            cum_select.push(format!("src.{dcn}"));
            cum_group.push(idx.to_string());
            idx += 1;
        }
        cum_select.push(format!("{agg_src} AS {}", metric.name));
    } else {
        let trunc = format!("DATE_TRUNC('{granularity}', spine.spine_time)");
        cum_select = vec![format!("{trunc} AS {time_out_col}")];
        cum_group = (1..=1 + dim_col_names.len())
            .map(|i| i.to_string())
            .collect();
        for dcn in &dim_col_names {
            cum_select.push(format!("src.{dcn}"));
        }
        cum_select.push(format!("{agg_src} AS {}", metric.name));
    }

    let mut cum_sql = format!(
        "SELECT {} FROM {spine_cte} AS spine \
         INNER JOIN {src_cte} AS src ON {join_cond}{cum_extra_join} \
         GROUP BY {}",
        cum_select.join(", "),
        cum_group.join(", ")
    );

    if !deferred_where_parts.is_empty() {
        // Wrap in an outer SELECT to apply deferred time-dimension filters.
        // The cumulative CTE output columns are: time_out_col, dim_col_names, metric.name.
        // The raw filters reference column names like "revenue_instance__ds__day" or
        // "{{ TimeDimension('metric_time', 'day') }}". Resolve them against the output names.
        let inner_name = format!("{}_inner", metric.name);
        ctes.push((inner_name.clone(), cum_sql));
        let outer_cols: Vec<String> = std::iter::once(time_out_col.to_string())
            .chain(dim_col_names.iter().cloned())
            .chain(std::iter::once(metric.name.clone()))
            .collect();
        let resolved_deferred: Vec<String> = deferred_where_parts
            .iter()
            .map(|f| {
                let mut r = f.clone();
                // Strip Jinja wrappers.
                r = r.replace("{{ ", "").replace(" }}", "");
                // Replace TimeDimension('name', 'grain') → output column name.
                while let Some(start) = r.find("TimeDimension('") {
                    let inner_start = start + 15;
                    if let Some(end) = r[inner_start..].find("')") {
                        let inner = &r[inner_start..inner_start + end];
                        let td_name = inner
                            .split(',')
                            .next()
                            .unwrap_or("metric_time")
                            .trim()
                            .trim_matches('\'');
                        let td_gran = inner
                            .split(',')
                            .nth(1)
                            .map(|s| s.trim().trim_matches('\'').trim_matches('"'))
                            .unwrap_or("day");
                        let col_name = format!("{td_name}__{td_gran}");
                        let col = outer_cols
                            .iter()
                            .find(|c| **c == col_name || **c == td_name)
                            .cloned()
                            .unwrap_or_else(|| time_out_col.to_string());
                        r.replace_range(start..inner_start + end + 2, &col);
                    } else {
                        break;
                    }
                }
                // Replace Dimension('entity__name') → output column.
                while let Some(start) = r.find("Dimension('") {
                    let inner_start = start + 11;
                    if let Some(end) = r[inner_start..].find("')") {
                        let dim_ref = &r[inner_start..inner_start + end];
                        let col = outer_cols
                            .iter()
                            .find(|c| c.as_str() == dim_ref)
                            .cloned()
                            .unwrap_or_else(|| dim_ref.to_string());
                        r.replace_range(start..inner_start + end + 2, &col);
                    } else {
                        break;
                    }
                }
                // Replace raw `name__granularity` references with the actual output col.
                // E.g., `revenue_instance__ds__day` → `revenue_instance__ds` when that's the output.
                for td in &time_dim_output_names {
                    if r.contains(td.as_str()) && !outer_cols.contains(td) {
                        r = r.replace(td.as_str(), time_out_col);
                    }
                }
                r
            })
            .collect();
        cum_sql = format!(
            "SELECT {} FROM {inner_name} WHERE {}",
            outer_cols.join(", "),
            resolved_deferred.join(" AND ")
        );
    }

    if has_non_default_grain {
        // Wrap the daily cumulative in a LAST_VALUE/AVG window function to pick the
        // representative value per coarser-grain bucket, then GROUP BY to dedup.
        let daily_cte = format!("{}_daily", metric.name);
        ctes.push((daily_cte.clone(), cum_sql));

        // Choose window function: LAST_VALUE for all-time, AVG for windowed/grain-to-date.
        let wfn = if cp.grain_to_date.is_some() {
            "FIRST_VALUE"
        } else if cp.window_count.is_some() {
            "AVG"
        } else {
            "LAST_VALUE"
        };

        // Build partition columns = all the coarser-grain time cols + dim cols.
        let mut partition_cols: Vec<String> = Vec::new();
        for (td_name, td_gran) in &time_dims {
            partition_cols.push(format!("{td_name}__{td_gran}"));
        }
        for dcn in &dim_col_names {
            partition_cols.push(dcn.clone());
        }
        let partition_clause = partition_cols.join(", ");
        let order_col = format!("{time_out_col}__day");

        let mut wfn_select: Vec<String> = Vec::new();
        for col in &partition_cols {
            wfn_select.push(col.clone());
        }
        wfn_select.push(format!(
            "{wfn}({metric_name}) OVER (PARTITION BY {partition_clause} ORDER BY {order_col} \
             ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING) AS {metric_name}",
            metric_name = metric.name,
        ));

        let wfn_sql = format!("SELECT {} FROM {daily_cte}", wfn_select.join(", "),);
        let wfn_cte = format!("{}_wfn", metric.name);
        ctes.push((wfn_cte.clone(), wfn_sql));

        // Dedup: GROUP BY all partition cols + metric value.
        let mut dedup_cols: Vec<String> = partition_cols.clone();
        dedup_cols.push(metric.name.clone());
        // Rename partition cols to match expected output names.
        let mut final_select: Vec<String> = Vec::new();
        for (td_name, td_gran) in &time_dims {
            final_select.push(format!("{td_name}__{td_gran}"));
        }
        for dcn in &dim_col_names {
            final_select.push(dcn.clone());
        }
        final_select.push(metric.name.clone());
        let group_indices: Vec<String> = (1..=dedup_cols.len()).map(|i| i.to_string()).collect();
        let dedup_sql = format!(
            "SELECT {} FROM {wfn_cte} GROUP BY {}",
            final_select.join(", "),
            group_indices.join(", ")
        );
        ctes.push((metric.name.clone(), dedup_sql));
    } else {
        ctes.push((metric.name.clone(), cum_sql));
    }
    Ok(())
}

/// Build a scoped model-alias map containing only one model (for CTE compilation).
fn scoped_aliases_for<'a>(
    ap: &AggParams,
    alias: &str,
    model: &'a ResolvedModel,
) -> HashMap<String, (String, &'a ResolvedModel)> {
    let mut m = HashMap::new();
    m.insert(ap.semantic_model.clone(), (alias.to_string(), model));
    m
}

/// Compile a conversion metric into CTEs.
///
/// Pattern:
///   {metric}_base  — raw rows from the base measure's model (entity key + time)
///   {metric}_conv  — raw rows from the conversion measure's model (entity key + time)
///   {metric}       — matched conversions joined by entity within the time window
///
/// For `conversion_rate`: matched / total_base
/// For `conversions`: COUNT of matched conversion events
#[allow(
    clippy::too_many_arguments,
    clippy::used_underscore_binding,
    clippy::cognitive_complexity
)]
fn compile_conversion_metric_cte(
    metric: &ResolvedMetric,
    _spec: &SemanticQuerySpec,
    all_metrics: &HashMap<String, ResolvedMetric>,
    model_aliases: &HashMap<String, (String, &ResolvedModel)>,
    _resolved_models: &HashMap<String, ResolvedModel>,
    _join_edges: &[JoinEdge],
    dialect: Dialect,
    ctes: &mut Vec<(String, String)>,
    all_time_spines: &[TimeSpine],
) -> Result<(), MetricFlowError> {
    let cp = metric.conversion_params.as_ref().ok_or_else(|| {
        MetricFlowError::Other(format!(
            "conversion metric {} has no conversion params",
            metric.name
        ))
    })?;

    // Resolve the base and conversion measures to get their models and columns.
    let base_metric = all_metrics.get(&cp.base_metric).ok_or_else(|| {
        MetricFlowError::Other(format!("base metric not resolved: {}", cp.base_metric))
    })?;
    let conv_metric = all_metrics.get(&cp.conversion_metric).ok_or_else(|| {
        MetricFlowError::Other(format!(
            "conversion metric not resolved: {}",
            cp.conversion_metric
        ))
    })?;

    let base_ap = base_metric
        .agg_params
        .as_ref()
        .ok_or_else(|| MetricFlowError::Other("base metric has no agg_params".into()))?;
    let conv_ap = conv_metric
        .agg_params
        .as_ref()
        .ok_or_else(|| MetricFlowError::Other("conversion metric has no agg_params".into()))?;

    let base_model = model_aliases.get(&base_ap.semantic_model).ok_or_else(|| {
        MetricFlowError::Other(format!("model not resolved: {}", base_ap.semantic_model))
    })?;
    let conv_model = model_aliases.get(&conv_ap.semantic_model).ok_or_else(|| {
        MetricFlowError::Other(format!("model not resolved: {}", conv_ap.semantic_model))
    })?;

    // Find the entity expression in each model.
    let base_entity_expr = base_model
        .1
        .entities
        .iter()
        .find(|e| e.name == cp.entity)
        .map(|e| e.expr.as_str())
        .unwrap_or(&cp.entity);
    let conv_entity_expr = conv_model
        .1
        .entities
        .iter()
        .find(|e| e.name == cp.entity)
        .map(|e| e.expr.as_str())
        .unwrap_or(&cp.entity);

    // Find the time dimension expression in each model.
    let base_time_dim = base_model
        .1
        .dimensions
        .iter()
        .find(|d| d.dimension_type == "time")
        .map(|d| d.expr.as_str())
        .unwrap_or("ds");
    let conv_time_dim = conv_model
        .1
        .dimensions
        .iter()
        .find(|d| d.dimension_type == "time")
        .map(|d| d.expr.as_str())
        .unwrap_or("ds");

    let base_relation = render_full_relation(base_model.1, dialect);
    let conv_relation = render_full_relation(conv_model.1, dialect);

    // Determine the time dimension granularity and group-by columns.
    let spec = _spec;
    let has_time_dim = spec
        .group_by
        .iter()
        .any(|gb| matches!(gb, GroupBySpec::TimeDimension { .. }));
    let granularity = spec
        .group_by
        .iter()
        .find_map(|gb| {
            if let GroupBySpec::TimeDimension { granularity, .. } = gb {
                Some(granularity.as_str())
            } else {
                None
            }
        })
        .unwrap_or("day");
    let _group_by_cols = group_by_output_cols(&spec.group_by);

    let custom_gran_info = if !is_standard_granularity(granularity) {
        find_custom_granularity_spine(all_time_spines, granularity)
            .map(|(spine, col)| (spine.clone(), col.to_string()))
    } else {
        None
    };

    // Build extra SELECT columns for constant properties.
    let const_conditions: Vec<String> = cp
        .constant_properties
        .iter()
        .map(|(base_prop, conv_prop)| format!("b.{base_prop} = c.{conv_prop}"))
        .collect();

    // Window condition for the join.
    let window_condition = match (&cp.window_count, &cp.window_granularity) {
        (Some(count), Some(gran)) => {
            let interval = render_interval(*count, gran, dialect);
            format!("b.metric_time <= c.metric_time AND b.metric_time > c.metric_time - {interval}")
        }
        _ => "b.metric_time <= c.metric_time".to_string(),
    };
    let mut join_conds = vec!["b.entity_key = c.entity_key".to_string(), window_condition];
    join_conds.extend(const_conditions);
    let _join_condition = join_conds.join(" AND ");

    // Resolve dimension columns for group-by (non-time).
    let dim_cols: Vec<(String, String)> = spec
        .group_by
        .iter()
        .filter_map(|gb| match gb {
            GroupBySpec::Dimension {
                entity: Some(e),
                name,
            } => {
                let dim_ref = format!("{e}__{name}");
                let base_col = base_model
                    .1
                    .dimensions
                    .iter()
                    .find(|d| d.name == *name || d.name == dim_ref)
                    .map(|d| d.expr.to_string())
                    .unwrap_or_else(|| name.clone());
                Some((dim_ref, base_col))
            }
            GroupBySpec::Dimension { entity: None, name } => Some((name.clone(), name.clone())),
            _ => None,
        })
        .collect();

    let opp_cte_name = format!("{}_opportunities", metric.name);
    let conv_matched_name = format!("{}_conversions", metric.name);

    // CTE 1: opportunities (base measure aggregated per group-by).
    let base_measure = qualify_measure_expr(&base_model.0, &base_ap.expr);
    let base_agg = render_agg_with_params(
        &base_ap.agg,
        &base_measure,
        dialect,
        base_ap.percentile,
        base_ap.use_discrete_percentile,
    );
    if has_time_dim {
        if let Some((ref cg_spine, ref cg_col)) = custom_gran_info {
            let spine_rel = match dialect {
                Dialect::Databricks => cg_spine.relation_name.replace('"', "`"),
                _ => cg_spine.relation_name.clone(),
            };
            let day_trunc =
                render_date_trunc("day", &format!("{}.{base_time_dim}", base_model.0), dialect);
            let mut opp_sel = vec![format!("ts_opp.{cg_col} AS metric_time__{granularity}")];
            for (out_name, src_col) in &dim_cols {
                opp_sel.push(format!("{}.{src_col} AS {out_name}", base_model.0));
            }
            opp_sel.push(format!("{base_agg} AS base_measure"));
            let indices: Vec<String> = (1..=1 + dim_cols.len()).map(|i| i.to_string()).collect();
            let opp_sql = format!(
                "SELECT {} FROM {base_relation} AS {} \
                 LEFT OUTER JOIN {spine_rel} AS ts_opp ON {day_trunc} = ts_opp.{} \
                 GROUP BY {}",
                opp_sel.join(", "),
                base_model.0,
                cg_spine.primary_column,
                indices.join(", ")
            );
            ctes.push((opp_cte_name.clone(), opp_sql));
        } else {
            let time_expr = render_date_trunc(
                granularity,
                &format!("{}.{base_time_dim}", base_model.0),
                dialect,
            );
            let mut opp_sel = vec![format!("{time_expr} AS metric_time")];
            for (out_name, src_col) in &dim_cols {
                opp_sel.push(format!("{}.{src_col} AS {out_name}", base_model.0));
            }
            opp_sel.push(format!("{base_agg} AS base_measure"));
            let indices: Vec<String> = (1..=1 + dim_cols.len()).map(|i| i.to_string()).collect();
            let opp_sql = format!(
                "SELECT {} FROM {base_relation} AS {} GROUP BY {}",
                opp_sel.join(", "),
                base_model.0,
                indices.join(", ")
            );
            ctes.push((opp_cte_name.clone(), opp_sql));
        }
    } else {
        let mut opp_sel: Vec<String> = Vec::new();
        for (out_name, src_col) in &dim_cols {
            opp_sel.push(format!("{}.{src_col} AS {out_name}", base_model.0));
        }
        opp_sel.push(format!("{base_agg} AS base_measure"));
        let group = if dim_cols.is_empty() {
            String::new()
        } else {
            format!(
                " GROUP BY {}",
                (1..=dim_cols.len())
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let opp_sql = format!(
            "SELECT {} FROM {base_relation} AS {}{group}",
            opp_sel.join(", "),
            base_model.0
        );
        ctes.push((opp_cte_name.clone(), opp_sql));
    }

    // CTE 2: conversions — join base events to conversion events, deduplicate with FIRST_VALUE.
    // For each conversion event, find the closest matching base event within the window.
    // Pattern: SELECT DISTINCT first_value(b.time) OVER (PARTITION BY c.time, c.entity ORDER BY b.time DESC), ..., c.measure
    let conv_measure = qualify_measure_expr(&conv_model.0, &conv_ap.expr);
    let base_rel = format!("{base_relation} AS b");
    let uuid_fn = match dialect {
        Dialect::DuckDB => "GEN_RANDOM_UUID()",
        _ => "UUID_STRING()",
    };
    let conv_rel = format!("(SELECT *, {uuid_fn} AS uuid FROM {conv_relation}) AS c");
    let mut join_on = vec![format!("b.{base_entity_expr} = c.{conv_entity_expr}")];
    match (&cp.window_count, &cp.window_granularity) {
        (Some(count), Some(gran)) => {
            let interval = render_interval(*count, gran, dialect);
            join_on.push(format!("b.{base_time_dim} <= c.{conv_time_dim}"));
            join_on.push(format!(
                "b.{base_time_dim} > c.{conv_time_dim} - {interval}"
            ));
        }
        _ => {
            join_on.push(format!("b.{base_time_dim} <= c.{conv_time_dim}"));
        }
    }
    for (base_prop, conv_prop) in &cp.constant_properties {
        join_on.push(format!("b.{base_prop} = c.{conv_prop}"));
    }
    let partition_by = format!("c.{conv_time_dim}, c.{conv_entity_expr}");
    let window_spec = format!(
        "PARTITION BY {partition_by} ORDER BY b.{base_time_dim} DESC ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING"
    );
    let mut dedup_sel = vec![
        format!("FIRST_VALUE(b.{base_time_dim}) OVER ({window_spec}) AS ds"),
        format!("FIRST_VALUE(b.{base_entity_expr}) OVER ({window_spec}) AS entity_key"),
    ];
    for (base_prop, _) in &cp.constant_properties {
        dedup_sel.push(format!(
            "FIRST_VALUE(b.{base_prop}) OVER ({window_spec}) AS {base_prop}"
        ));
    }
    // Include dimension columns in the dedup via FIRST_VALUE.
    for (out_name, src_col) in &dim_cols {
        dedup_sel.push(format!(
            "FIRST_VALUE(b.{src_col}) OVER ({window_spec}) AS {out_name}"
        ));
    }
    dedup_sel.push("c.uuid".to_string());
    dedup_sel.push(format!("{conv_measure} AS conv_measure"));
    let dedup_sql = format!(
        "SELECT DISTINCT {} FROM {base_rel} INNER JOIN {conv_rel} ON {}",
        dedup_sel.join(", "),
        join_on.join(" AND ")
    );

    // Aggregate conversions per group-by.
    let conv_agg = render_agg_with_params(
        &conv_ap.agg,
        "conv_measure",
        dialect,
        conv_ap.percentile,
        conv_ap.use_discrete_percentile,
    );
    let mut conv_sel: Vec<String> = Vec::new();
    let mut conv_extra_join = String::new();
    if has_time_dim {
        if let Some((ref cg_spine, ref cg_col)) = custom_gran_info {
            let spine_rel = match dialect {
                Dialect::Databricks => cg_spine.relation_name.replace('"', "`"),
                _ => cg_spine.relation_name.clone(),
            };
            conv_sel.push(format!("ts_conv.{cg_col} AS metric_time__{granularity}"));
            conv_extra_join = format!(
                " LEFT OUTER JOIN {spine_rel} AS ts_conv ON deduped.ds = ts_conv.{}",
                cg_spine.primary_column,
            );
        } else {
            let time_expr = render_date_trunc(granularity, "deduped.ds", dialect);
            conv_sel.push(format!("{time_expr} AS metric_time"));
        }
    }
    for (out_name, _) in &dim_cols {
        conv_sel.push(out_name.clone());
    }
    conv_sel.push(format!("{conv_agg} AS conv_measure"));
    let n_group = (if has_time_dim { 1 } else { 0 }) + dim_cols.len();
    let group = if n_group > 0 {
        format!(
            " GROUP BY {}",
            (1..=n_group)
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else {
        String::new()
    };
    let conv_sql = format!(
        "SELECT {} FROM ({dedup_sql}) deduped{conv_extra_join}{group}",
        conv_sel.join(", ")
    );
    ctes.push((conv_matched_name.clone(), conv_sql));

    // CTE 3: final metric — FULL OUTER JOIN opportunities and conversions.
    let cast_conv = render_cast_double("conv.conv_measure", dialect);
    let cast_base = render_cast_double("NULLIF(opp.base_measure, 0)", dialect);
    let metric_expr_str = match cp.calculation.as_str() {
        "conversions" => "conv.conv_measure".to_string(),
        _ => format!("{cast_conv} / {cast_base}"),
    };
    if has_time_dim || !dim_cols.is_empty() {
        let time_col = if custom_gran_info.is_some() {
            format!("metric_time__{granularity}")
        } else {
            "metric_time".to_string()
        };
        let mut final_sel = Vec::new();
        if has_time_dim {
            final_sel.push(format!(
                "COALESCE(opp.{time_col}, conv.{time_col}) AS {time_col}"
            ));
        }
        for (out_name, _) in &dim_cols {
            final_sel.push(format!(
                "COALESCE(opp.{out_name}, conv.{out_name}) AS {out_name}"
            ));
        }
        final_sel.push(format!("{metric_expr_str} AS {}", metric.name));
        let mut join_on_parts = Vec::new();
        if has_time_dim {
            join_on_parts.push(format!("opp.{time_col} = conv.{time_col}"));
        }
        for (out_name, _) in &dim_cols {
            join_on_parts.push(format!(
                "opp.{out_name} IS NOT DISTINCT FROM conv.{out_name}"
            ));
        }
        let final_sql = format!(
            "SELECT {} FROM {opp_cte_name} opp FULL OUTER JOIN {conv_matched_name} conv ON {}",
            final_sel.join(", "),
            join_on_parts.join(" AND ")
        );
        ctes.push((metric.name.clone(), final_sql));
    } else {
        let cast_conv_scalar = render_cast_double(
            &format!("(SELECT SUM(conv_measure) FROM {conv_matched_name})"),
            dialect,
        );
        let cast_base_scalar = render_cast_double(
            &format!("NULLIF((SELECT SUM(base_measure) FROM {opp_cte_name}), 0)"),
            dialect,
        );
        let scalar_expr = match cp.calculation.as_str() {
            "conversions" => format!("(SELECT SUM(conv_measure) FROM {conv_matched_name})"),
            _ => format!("{cast_conv_scalar} / {cast_base_scalar}"),
        };
        let final_sql = format!("SELECT {scalar_expr} AS {}", metric.name);
        ctes.push((metric.name.clone(), final_sql));
    }
    Ok(())
}

/// Add JOIN clauses for dimensions from other models.
///
/// Scans both the `group_by` specs and any filter strings (metric-level or
/// user-supplied) for entity-prefixed `Dimension('entity__name')` references,
/// then emits one LEFT JOIN per referenced model that is not already joined.
///
/// When multi-hop subqueries are provided (for chains like
/// `account_id__customer_id__dim` that diverge to different leaf models),
/// those are emitted as subquery joins and the corresponding dimensions are
/// skipped from flat join processing.
#[allow(clippy::too_many_arguments, clippy::cognitive_complexity)]
fn add_dimension_joins(
    spec: &SemanticQuerySpec,
    metric_filters: &[String],
    primary_model_name: &str,
    primary_alias: &str,
    model_aliases: &HashMap<String, (String, &ResolvedModel)>,
    join_edges: &[JoinEdge],
    dialect: Dialect,
    sql: &mut String,
    multi_hop_subqueries: &[MultiHopSubquery],
    // For SCD joins: the qualified fact-table time expression, e.g. `"a.ds"`.
    fact_time_expr: Option<&str>,
) {
    let mut joined: HashSet<String> = HashSet::new();
    joined.insert(primary_model_name.to_string());

    // Emit multi-hop subquery joins first.
    for mh in multi_hop_subqueries {
        let _ = write!(
            sql,
            " LEFT JOIN ({}) AS {} ON {} = {}.{}",
            mh.subquery_sql, mh.alias, mh.fact_join_expr, mh.alias, mh.subquery_join_col,
        );
    }

    // Collect (entity_name, dimension_name) pairs from group-by and filters.
    // We need both pieces to find the correct model: the entity tells us HOW
    // to join, and the dimension tells us WHICH model to join (the one that
    // actually owns the column).
    // Allow multiple entries per entity when different dimensions are needed.
    let mut needed: Vec<(String, Option<String>)> = Vec::new();

    for gb in &spec.group_by {
        match gb {
            GroupBySpec::Dimension {
                entity: Some(entity_name),
                name,
            } => {
                if !needed
                    .iter()
                    .any(|(e, d)| e == entity_name && d.as_deref() == Some(name.as_str()))
                {
                    needed.push((entity_name.clone(), Some(name.clone())));
                }
            }
            GroupBySpec::TimeDimension { name, .. } => {
                if let Some((entity_chain, _dim_name)) = name.rsplit_once("__") {
                    if entity_chain.contains("__") {
                        // Multi-hop: add each segment as a separate needed entity
                        // so intermediate JOINs are emitted.
                        for seg in entity_chain.split("__") {
                            if !needed.iter().any(|(e, _)| e == seg) {
                                needed.push((seg.to_string(), None));
                            }
                        }
                    } else if !needed.iter().any(|(e, _)| e == entity_chain) {
                        needed.push((entity_chain.to_string(), None));
                    }
                }
            }
            _ => {}
        }
    }

    // Extract entity prefixes from `Dimension('entity__dim')` in all filters.
    for filter in metric_filters.iter().chain(spec.where_filters.iter()) {
        let mut cursor = 0usize;
        while let Some(pos) = filter[cursor..].find("Dimension(") {
            let abs = cursor + pos;
            let preceded_by_alpha = abs > 0 && filter.as_bytes()[abs - 1].is_ascii_alphabetic();
            if preceded_by_alpha {
                cursor = abs + 10;
                continue;
            }
            let inner_start = abs + 10;
            if let Some(paren_end) = filter[inner_start..].find(')') {
                let dim_ref = filter[inner_start..inner_start + paren_end]
                    .trim()
                    .trim_matches('\'')
                    .trim_matches('"');
                if let Some((entity_name, dim_name)) = dim_ref.split_once("__") {
                    let entity_name = entity_name.to_string();
                    if !needed.iter().any(|(e, _)| *e == entity_name) {
                        needed.push((entity_name, Some(dim_name.to_string())));
                    }
                }
                cursor = inner_start + paren_end + 1;
            } else {
                break;
            }
        }
    }

    // Emit one LEFT JOIN per needed entity, skipping already-joined models.
    // When a dimension name is known, prefer the model that actually owns that
    // dimension column (not just any model that shares the entity name).
    for (entity_name, dim_name) in &needed {
        // Skip dimensions that are handled by multi-hop subqueries.
        if let Some(dn) = dim_name {
            if multi_hop_subqueries
                .iter()
                .any(|mh| mh.dim_columns.contains_key(dn.as_str()))
            {
                continue;
            }
        }

        // For multi-hop entity chains (e.g. "account_id__customer_id"), use
        // the last entity for model matching.
        let entity_segments: Vec<&str> = entity_name.split("__").collect();
        let target_entity = *entity_segments.last().unwrap_or(&entity_name.as_str());

        // If a model already in `joined` has both the entity AND the dimension,
        // no additional join is needed (the column is already accessible).
        let already_satisfied = joined.iter().any(|jm| {
            if let Some((_alias, model)) = model_aliases.get(jm) {
                let has_entity = model.entities.iter().any(|e| e.name == target_entity);
                if dim_name.is_none() {
                    let is_primary = model.entities.iter().any(|e| {
                        e.name == target_entity
                            && matches!(e.entity_type.as_str(), "primary" | "unique" | "natural")
                    }) || model.primary_entity.as_deref() == Some(target_entity);
                    return is_primary;
                }
                let has_dim = dim_name.as_ref().is_none_or(|dn| {
                    model.dimensions.iter().any(|d| d.name == *dn)
                        || model.entities.iter().any(|e| e.name == *dn)
                });
                has_entity && has_dim
            } else {
                false
            }
        });
        if already_satisfied {
            continue;
        }

        // Find the best model: one that has both the entity AND the dimension.
        // Fall back to any model with the entity if no dimension match.
        // For entity-prefixed time dims (dim_name=None), prefer the model
        // where the entity is primary/unique.
        let mut best: Option<&String> = None;
        let mut fallback: Option<&String> = None;
        for (model_name, (_alias, model)) in model_aliases {
            if joined.contains(model_name) {
                continue;
            }
            let has_entity = model.entities.iter().any(|e| e.name == target_entity);
            if !has_entity {
                continue;
            }
            if dim_name.is_none() {
                let is_primary = model.entities.iter().any(|e| {
                    e.name == target_entity
                        && matches!(e.entity_type.as_str(), "primary" | "unique" | "natural")
                }) || model.primary_entity.as_deref() == Some(target_entity);
                if is_primary {
                    best = Some(model_name);
                    break;
                }
            }
            if let Some(dn) = dim_name {
                let has_dim = model.dimensions.iter().any(|d| d.name == *dn);
                if has_dim {
                    best = Some(model_name);
                    break;
                }
            }
            if fallback.is_none() {
                fallback = Some(model_name);
            }
        }
        let target = best.or(fallback);
        if let Some(model_name) = target {
            if let Some(path) = find_join_path(join_edges, primary_model_name, model_name) {
                // Emit joins for the entire path (may be multi-hop).
                for edge in &path {
                    if joined.contains(&edge.to_model) {
                        continue;
                    }
                    let left_alias = if edge.from_model == primary_model_name {
                        primary_alias
                    } else {
                        model_aliases
                            .get(&edge.from_model)
                            .map(|(a, _)| a.as_str())
                            .unwrap_or(primary_alias)
                    };
                    let (alias, model) = &model_aliases[&edge.to_model];
                    let join_relation = render_full_relation(model, dialect);
                    let _ = write!(
                        sql,
                        " LEFT JOIN {join_relation} AS {alias} ON {left_alias}.{} = {alias}.{}",
                        edge.from_expr, edge.to_expr,
                    );
                    // SCD temporal range condition: fact.time >= scd.valid_from AND
                    // (fact.time < scd.valid_to OR scd.valid_to IS NULL)
                    if let (Some(fte), Some(vf), Some(vt)) = (
                        fact_time_expr,
                        model.scd_valid_from.as_deref(),
                        model.scd_valid_to.as_deref(),
                    ) {
                        let _ = write!(
                            sql,
                            " AND {fte} >= {alias}.{vf} AND ({fte} < {alias}.{vt} OR {alias}.{vt} IS NULL)",
                        );
                    }
                    joined.insert(edge.to_model.clone());
                }
            }
        }
    }
}

/// Build the final SQL from CTEs, combining all metric results.
#[allow(unused_variables, clippy::cognitive_complexity)]
fn build_final_sql(
    spec: &SemanticQuerySpec,
    ctes: &[(String, String)],
    dialect: Dialect,
    all_metrics: &HashMap<String, ResolvedMetric>,
    time_spine: Option<&TimeSpine>,
    all_time_spines: &[TimeSpine],
) -> Result<String, MetricFlowError> {
    if ctes.is_empty() {
        return Err(MetricFlowError::Other(
            "no metrics compiled — nothing to query".into(),
        ));
    }

    let mut sql = String::new();

    // WITH clause.
    let _ = writeln!(sql, "WITH");
    for (i, (name, cte_sql)) in ctes.iter().enumerate() {
        if i > 0 {
            let _ = writeln!(sql, ",");
        }
        let _ = write!(sql, "  {name} AS (\n    {cte_sql}\n  )");
    }
    let _ = writeln!(sql);

    // Final SELECT — reference the last CTE for each top-level metric.
    let group_by_cols = group_by_output_cols(&spec.group_by);

    // Check if any top-level metric needs join_to_timespine.
    let needs_spine_join = spec
        .metrics
        .iter()
        .any(|m| all_metrics.get(m).is_some_and(|rm| rm.join_to_timespine));

    // Find the time column name for spine joins.
    let spine_time_col = group_by_cols
        .iter()
        .find(|c| c.contains("metric_time") || c.contains("__ds"))
        .cloned();

    // If join_to_timespine is needed, wrap the metric query in a LEFT JOIN from spine.
    // Pick the best time spine for the query granularity (subdaily queries need an hourly spine).
    let query_gran_for_spine = spine_time_col
        .as_ref()
        .and_then(|time_col| {
            spec.group_by.iter().find_map(|gb| {
                if let GroupBySpec::TimeDimension {
                    name, granularity, ..
                } = gb
                {
                    if name == "metric_time" || time_col.contains(name) {
                        return Some(granularity.as_str());
                    }
                }
                None
            })
        })
        .unwrap_or("day");
    let effective_spine = if !all_time_spines.is_empty() {
        pick_time_spine_for_granularity(all_time_spines, query_gran_for_spine)
    } else {
        time_spine
    };
    let did_spine_join = needs_spine_join
        && spine_time_col.is_some()
        && effective_spine.is_some()
        && spec.metrics.len() == 1;
    let mut spine_ref_for_where: Option<String> = None;
    if did_spine_join {
        let ts = effective_spine.unwrap();
        let time_col = spine_time_col.as_ref().unwrap();

        // Determine the query granularity from the group_by.
        let query_gran = spec
            .group_by
            .iter()
            .find_map(|gb| {
                if let GroupBySpec::TimeDimension {
                    name, granularity, ..
                } = gb
                {
                    if name == "metric_time" || time_col.contains(name) {
                        return Some(granularity.as_str());
                    }
                }
                None
            })
            .unwrap_or("day");

        let spine_relation = match dialect {
            Dialect::Databricks => ts.relation_name.replace('"', "`"),
            _ => ts.relation_name.clone(),
        };
        let spine_col = &ts.primary_column;
        let is_custom_gran = !is_standard_granularity(query_gran);
        let subdaily = matches!(query_gran, "hour" | "minute" | "second");
        let target_type = if subdaily { "TIMESTAMP" } else { "DATE" };
        let needs_trunc = !is_custom_gran && query_gran != ts.primary_granularity.as_str();

        // The spine column expression used in SELECT and ON.
        // When the spine is a direct table, reference `spine.col`.
        // When it's a DISTINCT subquery, the output is already `time_col`.
        // Build optional WHERE clause for the spine: apply time_constraint and
        // where_filters that reference metric_time so the spine only spans the
        // constrained range (otherwise LEFT JOIN produces a row per spine day).
        let mut spine_where_parts: Vec<String> = Vec::new();
        if let Some((start, end)) = &spec.time_constraint {
            let has_time = end.contains(' ') || end.contains('T');
            if has_time && subdaily {
                // For subdaily with sub-granularity precision, truncate start DOWN
                // and round end UP to the query granularity.
                spine_where_parts.push(format!(
                    "CAST(t.{spine_col} AS TIMESTAMP) >= DATE_TRUNC('{query_gran}', CAST('{start}' AS TIMESTAMP))"
                ));
                spine_where_parts.push(format!(
                    "CAST(t.{spine_col} AS TIMESTAMP) < DATE_TRUNC('{query_gran}', CAST('{end}' AS TIMESTAMP)) + INTERVAL '1 {query_gran}'"
                ));
            } else if has_time {
                spine_where_parts.push(format!(
                    "CAST(t.{spine_col} AS TIMESTAMP) >= CAST('{start}' AS TIMESTAMP)"
                ));
                spine_where_parts.push(format!(
                    "CAST(t.{spine_col} AS TIMESTAMP) <= CAST('{end}' AS TIMESTAMP)"
                ));
            } else {
                spine_where_parts.push(format!(
                    "CAST(t.{spine_col} AS TIMESTAMP) >= CAST('{start}' AS TIMESTAMP)"
                ));
                spine_where_parts.push(format!(
                    "CAST(t.{spine_col} AS TIMESTAMP) < CAST('{end}' AS TIMESTAMP) + INTERVAL '1 day'"
                ));
            }
        }
        for wf in &spec.where_filters {
            if wf.contains("metric_time") || wf.contains("TimeDimension") {
                let resolved = wf
                    .replace(
                        "{{ TimeDimension('metric_time', 'day') }}",
                        &format!("t.{spine_col}"),
                    )
                    .replace(
                        "{{ TimeDimension('metric_time') }}",
                        &format!("t.{spine_col}"),
                    );
                if resolved.contains(&format!("t.{spine_col}")) {
                    spine_where_parts.push(resolved);
                }
            }
        }
        let spine_where = if spine_where_parts.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", spine_where_parts.join(" AND "))
        };

        let (spine_from, spine_ref) = if is_custom_gran {
            if let Some((cg_spine, cg_col)) =
                find_custom_granularity_spine(all_time_spines, query_gran)
            {
                let cg_spine_rel = match dialect {
                    Dialect::Databricks => cg_spine.relation_name.replace('"', "`"),
                    _ => cg_spine.relation_name.clone(),
                };
                (
                    format!(
                        "(SELECT {cg_col} AS {time_col} FROM {cg_spine_rel} AS t{spine_where} GROUP BY {cg_col}) AS spine"
                    ),
                    format!("spine.{time_col}"),
                )
            } else {
                let col_cast =
                    render_type_cast(&format!("spine.{spine_col}"), target_type, dialect);
                (format!("{spine_relation} AS spine"), col_cast)
            }
        } else if needs_trunc || !spine_where.is_empty() {
            let raw_trunc = if needs_trunc {
                let raw = format!("DATE_TRUNC('{query_gran}', t.{spine_col})");
                render_type_cast(&raw, target_type, dialect)
            } else {
                render_type_cast(&format!("t.{spine_col}"), target_type, dialect)
            };
            let distinct_kw = if needs_trunc { "DISTINCT " } else { "" };
            (
                format!(
                    "(SELECT {distinct_kw}{raw_trunc} AS {time_col} FROM {spine_relation} AS t{spine_where}) AS spine"
                ),
                format!("spine.{time_col}"),
            )
        } else {
            let col_cast = render_type_cast(&format!("spine.{spine_col}"), target_type, dialect);
            (format!("{spine_relation} AS spine"), col_cast)
        };

        spine_ref_for_where = Some(spine_ref.clone());

        // Build metric select: COALESCE for fill_nulls_with metrics.
        let mut select_parts: Vec<String> = Vec::new();
        select_parts.push(format!("{spine_ref} AS {time_col}"));
        for col in &group_by_cols {
            if col == time_col {
                continue;
            }
            select_parts.push(format!("metric_cte.{col}"));
        }
        for metric_name in &spec.metrics {
            if let Some(fill) = all_metrics.get(metric_name).and_then(|m| m.fill_nulls_with) {
                select_parts.push(format!(
                    "COALESCE(metric_cte.{metric_name}, {fill}) AS {metric_name}"
                ));
            } else {
                select_parts.push(format!("metric_cte.{metric_name}"));
            }
        }

        let _ = writeln!(sql, "SELECT {}", select_parts.join(", "));
        let _ = writeln!(sql, "FROM {spine_from}");

        let join_cond = format!("{spine_ref} = metric_cte.{time_col}");
        let _ = writeln!(
            sql,
            "LEFT JOIN {} AS metric_cte ON {join_cond}",
            spec.metrics[0]
        );
    } else if spec.metrics.len() == 1 {
        let _ = write!(sql, "SELECT *\nFROM {}", spec.metrics[0]);
    } else {
        // Multiple metrics: FULL OUTER JOIN their CTEs on group-by columns.
        // When a metric has join_to_timespine, wrap it in a spine-join subquery.
        let build_spine_subquery = |metric_name: &str, time_col: &str| -> Option<String> {
            let m = all_metrics.get(metric_name)?;
            if !m.join_to_timespine {
                return None;
            }
            let ts = effective_spine?;
            let query_gran = spec
                .group_by
                .iter()
                .find_map(|gb| {
                    if let GroupBySpec::TimeDimension {
                        name, granularity, ..
                    } = gb
                    {
                        if name == "metric_time" || time_col.contains(name) {
                            return Some(granularity.as_str());
                        }
                    }
                    None
                })
                .unwrap_or("day");
            let spine_relation = match dialect {
                Dialect::Databricks => ts.relation_name.replace('"', "`"),
                _ => ts.relation_name.clone(),
            };
            let spine_col = &ts.primary_column;
            let subdaily = matches!(query_gran, "hour" | "minute" | "second");
            let target_type = if subdaily { "TIMESTAMP" } else { "DATE" };
            let needs_trunc = query_gran != ts.primary_granularity.as_str();
            let spine_expr = if needs_trunc {
                let raw = format!("DATE_TRUNC('{query_gran}', t.{spine_col})");
                render_type_cast(&raw, target_type, dialect)
            } else {
                render_type_cast(&format!("t.{spine_col}"), target_type, dialect)
            };
            let distinct_kw = if needs_trunc { "DISTINCT " } else { "" };
            let mut metric_col = format!("mc.{metric_name}");
            if let Some(fill) = m.fill_nulls_with {
                metric_col = format!("COALESCE(mc.{metric_name}, {fill})");
            }
            let mut other_cols = String::new();
            for col in &group_by_cols {
                if col == time_col {
                    continue;
                }
                other_cols.push_str(&format!(", mc.{col}"));
            }
            Some(format!(
                "(SELECT spine.{time_col}{other_cols}, {metric_col} AS {metric_name} FROM (SELECT {distinct_kw}{spine_expr} AS {time_col} FROM {spine_relation} AS t) AS spine LEFT JOIN {metric_name} AS mc ON spine.{time_col} = mc.{time_col})"
            ))
        };

        let first = &spec.metrics[0];
        let first_alias = format!("{first}_final");
        let time_col_ref = spine_time_col.as_deref().unwrap_or("metric_time");

        let mut select_parts: Vec<String> = Vec::new();
        for col in &group_by_cols {
            let coalesce_parts: Vec<String> = spec
                .metrics
                .iter()
                .map(|m| format!("{m}_final.{col}"))
                .collect();
            if coalesce_parts.len() > 1 {
                select_parts.push(format!("COALESCE({}) AS {col}", coalesce_parts.join(", ")));
            } else {
                select_parts.push(format!("{first_alias}.{col}"));
            }
        }

        for metric_name in &spec.metrics {
            let alias = format!("{metric_name}_final");
            if let Some(fill) = all_metrics.get(metric_name).and_then(|m| m.fill_nulls_with) {
                select_parts.push(format!(
                    "COALESCE({alias}.{metric_name}, {fill}) AS {metric_name}"
                ));
            } else {
                select_parts.push(format!("{alias}.{metric_name}"));
            }
        }

        let _ = writeln!(sql, "SELECT {}", select_parts.join(", "));
        let first_from = if needs_spine_join {
            build_spine_subquery(first, time_col_ref).unwrap_or_else(|| first.to_string())
        } else {
            first.to_string()
        };
        let _ = writeln!(sql, "FROM {first_from} AS {first_alias}");

        let mut joined_aliases: Vec<String> = vec![first_alias.clone()];
        for metric_name in spec.metrics.iter().skip(1) {
            let alias = format!("{metric_name}_final");
            let join_conditions: Vec<String> = group_by_cols
                .iter()
                .map(|col| {
                    if joined_aliases.len() == 1 {
                        format!(
                            "{}.{col} IS NOT DISTINCT FROM {alias}.{col}",
                            joined_aliases[0]
                        )
                    } else {
                        let coalesce = joined_aliases
                            .iter()
                            .map(|a| format!("{a}.{col}"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!("COALESCE({coalesce}) IS NOT DISTINCT FROM {alias}.{col}")
                    }
                })
                .collect();

            let from_ref = if needs_spine_join {
                build_spine_subquery(metric_name, time_col_ref)
                    .unwrap_or_else(|| metric_name.to_string())
            } else {
                metric_name.to_string()
            };

            if join_conditions.is_empty() {
                let _ = writeln!(sql, "CROSS JOIN {from_ref} AS {alias}");
            } else {
                let _ = writeln!(
                    sql,
                    "FULL OUTER JOIN {from_ref} AS {alias} ON {}",
                    join_conditions.join(" AND ")
                );
            }
            joined_aliases.push(alias);
        }
    }

    // WHERE filters: for spine-joined queries, non-time where_filters need to be
    // applied at the outer level so they filter the spine result.
    let mut outer_where_parts: Vec<String> = Vec::new();

    if did_spine_join {
        for wf in &spec.where_filters {
            if !wf.contains("metric_time") && !wf.contains("TimeDimension") {
                // For the outer query, resolve Dimension() references to their
                // output column names (group-by aliases) rather than source columns.
                // Only apply if ALL referenced dimensions are in the group_by output.
                let mut resolved = wf.clone();
                let mut all_in_output = true;
                while let Some(start) = resolved.find("{{ Dimension('") {
                    let inner_start = start + "{{ Dimension('".len();
                    if let Some(end) = resolved[inner_start..].find("') }}") {
                        let dim_ref = &resolved[inner_start..inner_start + end];
                        if !group_by_cols.iter().any(|c| c == dim_ref) {
                            all_in_output = false;
                            break;
                        }
                        let out_col = dim_ref.to_string();
                        resolved = format!(
                            "{}{out_col}{}",
                            &resolved[..start],
                            &resolved[inner_start + end + "') }}".len()..]
                        );
                    } else {
                        break;
                    }
                }
                if all_in_output {
                    let resolved = resolved
                        .replace("{{ ", "")
                        .replace(" }}", "")
                        .replace("{{", "")
                        .replace("}}", "");
                    outer_where_parts.push(resolved);
                }
            }
        }
    }

    // Time constraint: applied at the outer level when a time column exists in the output.
    // For multi-metric queries with FULL OUTER JOIN, the outer WHERE must use an
    // unambiguous column. When the output time column is metric_time, use COALESCE
    // across all metrics. When it's an entity-prefixed dimension (e.g. listing__ds),
    // skip the outer constraint — each CTE already filters on the fact table's time.
    let outer_time_col = group_by_cols
        .iter()
        .find(|c| c.contains("metric_time") || c.contains("__ds"))
        .map(|s| s.as_str());
    if let (Some((start, end)), Some(time_col_name)) = (&spec.time_constraint, outer_time_col) {
        let is_metric_time = time_col_name.contains("metric_time");
        let skip_outer_time = spec.metrics.len() > 1 && !is_metric_time;
        if !skip_outer_time {
            let time_col = if let Some(ref sref) = spine_ref_for_where {
                sref.clone()
            } else if spec.metrics.len() > 1 {
                let coalesce_parts: Vec<String> = spec
                    .metrics
                    .iter()
                    .map(|m| format!("{m}_final.{time_col_name}"))
                    .collect();
                format!("COALESCE({})", coalesce_parts.join(", "))
            } else {
                time_col_name.to_string()
            };
            let has_time = end.contains(' ') || end.contains('T');
            let subdaily_outer = has_time && query_gran_for_spine != "day";
            let (start_expr, end_op, end_expr) = if subdaily_outer {
                (
                    format!("DATE_TRUNC('{query_gran_for_spine}', CAST('{start}' AS TIMESTAMP))"),
                    "<",
                    format!(
                        "DATE_TRUNC('{query_gran_for_spine}', CAST('{end}' AS TIMESTAMP)) + INTERVAL '1 {query_gran_for_spine}'"
                    ),
                )
            } else if has_time {
                (
                    format!("CAST('{start}' AS TIMESTAMP)"),
                    "<=",
                    format!("CAST('{end}' AS TIMESTAMP)"),
                )
            } else {
                (
                    format!("CAST('{start}' AS TIMESTAMP)"),
                    "<",
                    format!("CAST('{end}' AS TIMESTAMP) + INTERVAL '1 day'"),
                )
            };
            outer_where_parts.push(format!(
                "CAST({time_col} AS TIMESTAMP) >= {start_expr} AND CAST({time_col} AS TIMESTAMP) {end_op} {end_expr}"
            ));
        }
    }

    if !outer_where_parts.is_empty() {
        let _ = write!(sql, "\nWHERE {}", outer_where_parts.join(" AND "));
    }

    // ORDER BY.
    if !spec.order_by.is_empty() {
        let order_parts: Vec<String> = spec
            .order_by
            .iter()
            .map(|o| {
                let col = resolve_order_by_col(&o.name, &spec.group_by);
                if o.descending {
                    format!("{col} DESC")
                } else {
                    format!("{col} ASC")
                }
            })
            .collect();
        let _ = write!(sql, "\nORDER BY {}", order_parts.join(", "));
    }

    // LIMIT.
    if let Some(limit) = spec.limit {
        let _ = write!(sql, "\nLIMIT {limit}");
    }

    Ok(sql)
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_agg() {
        assert_eq!(
            render_agg("sum", "o.amount", Dialect::DuckDB),
            "SUM(o.amount)"
        );
        assert_eq!(
            render_agg("count_distinct", "c.customer_id", Dialect::Snowflake),
            "COUNT(DISTINCT c.customer_id)"
        );
        assert_eq!(
            render_agg("average", "o.amount", Dialect::DuckDB),
            "AVG(o.amount)"
        );
    }

    #[test]
    fn test_render_date_trunc() {
        assert_eq!(
            render_date_trunc("day", "o.order_date", Dialect::DuckDB),
            "DATE_TRUNC('day', o.order_date)::DATE"
        );
        assert_eq!(
            render_date_trunc("week", "o.order_date", Dialect::Snowflake),
            "DATE_TRUNC('week', o.order_date)::DATE"
        );
    }

    #[test]
    fn test_parse_metric_filters() {
        let json = r#"{"where_filters": [{"where_sql_template": "{{ Dimension('order_id__status') }} = 'completed'"}]}"#;
        let filters = parse_metric_filters(json);
        assert_eq!(filters.len(), 1);
        assert!(filters[0].contains("Dimension('order_id__status')"));
    }

    #[test]
    fn test_parse_metric_filters_empty() {
        assert!(parse_metric_filters("").is_empty());
        assert!(parse_metric_filters("null").is_empty());
    }

    #[test]
    fn test_resolve_time_dimension_ref_with_entity_prefix() {
        // TimeDimension('user_account_activity__date_day', 'day') should resolve
        // to `alias.date_day` via entity prefix stripping, not fall back to the
        // raw string `user_account_activity__date_day` as a column name.
        let model = ResolvedModel {
            name: "fct_activities".into(),
            relation_name: "\"db\".\"main\".\"fct_activities\"".into(),
            alias: "fct_activities".into(),
            schema_name: "main".into(),
            database: "db".into(),
            primary_entity: None,
            entities: vec![EntityDef {
                name: "user_account_activity".into(),
                entity_type: "primary".into(),
                expr: "activity_id".into(),
            }],
            dimensions: vec![DimensionDef {
                name: "date_day".into(),
                dimension_type: "time".into(),
                expr: "date_day".into(),
                time_granularity: Some("day".into()),
                is_partition: false,
            }],
            scd_valid_from: None,
            scd_valid_to: None,
        };

        let mut aliases: HashMap<String, (String, &ResolvedModel)> = HashMap::new();
        aliases.insert("fct_activities".into(), ("f".into(), &model));

        // Entity-prefixed form: must resolve to f.date_day, not the raw name.
        let resolved = resolve_time_dimension_ref(
            "user_account_activity__date_day",
            "day",
            &aliases,
            Dialect::Snowflake,
            "fct_activities",
        );
        assert_eq!(resolved, "DATE_TRUNC('day', f.date_day)::DATE");

        // Plain form (no entity prefix) should still work.
        let resolved_plain = resolve_time_dimension_ref(
            "date_day",
            "day",
            &aliases,
            Dialect::Snowflake,
            "fct_activities",
        );
        assert_eq!(resolved_plain, "DATE_TRUNC('day', f.date_day)::DATE");
    }

    #[test]
    fn test_resolve_where_filter() {
        // Build a simple model alias map.
        let model = ResolvedModel {
            name: "orders".into(),
            relation_name: "\"db\".\"main\".\"orders\"".into(),
            alias: "orders".into(),
            schema_name: "main".into(),
            database: "db".into(),
            primary_entity: None,
            entities: vec![EntityDef {
                name: "order_id".into(),
                entity_type: "primary".into(),
                expr: "order_id".into(),
            }],
            dimensions: vec![DimensionDef {
                name: "status".into(),
                dimension_type: "categorical".into(),
                expr: "status".into(),
                time_granularity: None,
                is_partition: false,
            }],
            scd_valid_from: None,
            scd_valid_to: None,
        };

        let mut aliases: HashMap<String, (String, &ResolvedModel)> = HashMap::new();
        aliases.insert("orders".into(), ("o".into(), &model));

        let resolved = resolve_where_filter(
            "{{ Dimension('order_id__status') }} = 'completed'",
            &aliases,
            Dialect::DuckDB,
            "orders",
        );
        assert_eq!(resolved, "o.status = 'completed'");
    }

    #[test]
    fn test_find_join_path_bidirectional() {
        let edges = vec![
            JoinEdge {
                from_model: "order_items".into(),
                to_model: "orders".into(),
                from_expr: "order_id".into(),
                to_expr: "order_id".into(),
                entity_name: "order".into(),
            },
            JoinEdge {
                from_model: "orders".into(),
                to_model: "order_items".into(),
                from_expr: "order_id".into(),
                to_expr: "order_id".into(),
                entity_name: "order".into(),
            },
        ];
        // FK→PK direction
        assert!(find_join_path(&edges, "order_items", "orders").is_some());
        // PK→FK direction (was broken before bidirectional edges)
        assert!(find_join_path(&edges, "orders", "order_items").is_some());
    }

    // ── Helpers for compile() integration tests ──────────────────────────────

    /// Minimal MetricStore backed by in-memory vecs — no database required.
    struct MockStore {
        metrics: Vec<RawMetricRow>,
        models: Vec<RawModelRow>,
        entities: Vec<(String, Vec<RawEntityRow>)>, // (unique_id, rows)
        dimensions: Vec<(String, Vec<RawDimensionRow>)>, // (unique_id, rows)
        join_graph: Vec<RawJoinGraphRow>,
    }

    impl MockStore {
        fn new() -> Self {
            Self {
                metrics: vec![],
                models: vec![],
                entities: vec![],
                dimensions: vec![],
                join_graph: vec![],
            }
        }
    }

    impl MetricStore for MockStore {
        fn lookup_metric(&mut self, name: &str) -> Result<Option<RawMetricRow>, MetricFlowError> {
            Ok(self.metrics.iter().find(|m| m.name == name).cloned())
        }
        fn list_metric_names(&mut self) -> Result<Vec<String>, MetricFlowError> {
            Ok(self.metrics.iter().map(|m| m.name.clone()).collect())
        }
        fn lookup_semantic_model(
            &mut self,
            name: &str,
        ) -> Result<Option<RawModelRow>, MetricFlowError> {
            Ok(self.models.iter().find(|m| m.name == name).cloned())
        }
        fn lookup_model_entities(
            &mut self,
            unique_id: &str,
        ) -> Result<Vec<RawEntityRow>, MetricFlowError> {
            Ok(self
                .entities
                .iter()
                .find(|(id, _)| id == unique_id)
                .map(|(_, rows)| rows.clone())
                .unwrap_or_default())
        }
        fn lookup_model_dimensions(
            &mut self,
            unique_id: &str,
        ) -> Result<Vec<RawDimensionRow>, MetricFlowError> {
            Ok(self
                .dimensions
                .iter()
                .find(|(id, _)| id == unique_id)
                .map(|(_, rows)| rows.clone())
                .unwrap_or_default())
        }
        fn lookup_all_join_graph_entities(
            &mut self,
        ) -> Result<Vec<RawJoinGraphRow>, MetricFlowError> {
            Ok(self.join_graph.clone())
        }
        fn find_model_for_entity(
            &mut self,
            entity_name: &str,
            primary_or_unique_only: bool,
        ) -> Result<Option<String>, MetricFlowError> {
            Ok(self
                .join_graph
                .iter()
                .find(|r| {
                    r.entity_name == entity_name
                        && (!primary_or_unique_only
                            || r.entity_type == "primary"
                            || r.entity_type == "unique")
                })
                .map(|r| r.model_name.clone()))
        }
        fn check_entity_in_model(
            &mut self,
            model_name: &str,
            entity_name: &str,
        ) -> Result<bool, MetricFlowError> {
            Ok(self
                .join_graph
                .iter()
                .any(|r| r.model_name == model_name && r.entity_name == entity_name))
        }
        fn lookup_time_spine(&mut self) -> Result<Option<RawTimeSpineRow>, MetricFlowError> {
            Ok(None)
        }
    }

    // ── Bug fix: measure expr fallback to type_params.expr ───────────────────

    /// Regression: some manifest formats store the measure column at
    /// `type_params.expr` rather than `type_params.metric_aggregation_params.expr`.
    /// The compiler must fall back to `type_params.expr` so it generates
    /// `COUNT(DISTINCT alias.col)` instead of `COUNT(DISTINCT alias.)`.
    #[test]
    fn test_measure_expr_fallback_to_type_params_expr() {
        let mut store = MockStore::new();

        // type_params with expr at the top level (not inside metric_aggregation_params).
        let type_params = r#"{
            "expr": "customer_user_id",
            "metric_aggregation_params": {
                "semantic_model": "fct_users",
                "agg": "count_distinct",
                "agg_time_dimension": "created_at"
            }
        }"#;

        store.metrics.push(RawMetricRow {
            name: "active_users".into(),
            metric_type: "simple".into(),
            description: String::new(),
            type_params: type_params.into(),
            metric_filter: String::new(),
            time_granularity: None,
        });
        store.models.push(RawModelRow {
            name: "fct_users".into(),
            node_relation: r#""db"."main"."fct_users""#.into(),
            primary_entity: "user".into(),
            unique_id: "semantic_model.fct_users".into(),
            ..Default::default()
        });
        store.entities.push((
            "semantic_model.fct_users".into(),
            vec![RawEntityRow {
                name: "user".into(),
                entity_type: "primary".into(),
                expr: "user_id".into(),
            }],
        ));
        store.dimensions.push((
            "semantic_model.fct_users".into(),
            vec![RawDimensionRow {
                name: "created_at".into(),
                dimension_type: "time".into(),
                expr: "created_at".into(),
                time_granularity: "day".into(),
                is_partition: false,
            }],
        ));
        store.join_graph.push(RawJoinGraphRow {
            model_name: "fct_users".into(),
            entity_name: "user".into(),
            entity_type: "primary".into(),
            expr: "user_id".into(),
        });

        let spec = SemanticQuerySpec {
            metrics: vec!["active_users".into()],
            group_by: vec![],
            where_filters: vec![],
            order_by: vec![],
            limit: None,
            time_constraint: None,
            apply_group_by: true,
        };

        let sql = compile(&mut store, &spec, Dialect::DuckDB).unwrap();
        // Must contain the column name, not a bare `f.` with nothing after it.
        assert!(
            sql.contains("COUNT(DISTINCT f.customer_user_id)"),
            "expected COUNT(DISTINCT f.customer_user_id) but got:\n{sql}"
        );
        assert!(
            !sql.contains("COUNT(DISTINCT f.)"),
            "bare `f.` with no column name should not appear:\n{sql}"
        );
    }

    // ── Bug fix: joins generated for dimensions in metric filters ────────────

    /// Regression: `add_dimension_joins` previously only generated JOINs for
    /// models referenced in `group_by`. A model referenced only in a metric
    /// filter (`Dimension('entity__col')`) was assigned an alias in the SQL but
    /// never joined, producing invalid SQL like `WHERE f3.col = X` with no
    /// corresponding `LEFT JOIN`.
    ///
    /// This must use a **derived** metric so the compiler takes the CTE path
    /// and calls `add_dimension_joins` — the function that was actually fixed.
    /// Simple metrics take `compile_simple_metrics`, which already had its own
    /// filter-scanning join logic and would not expose the bug.
    #[test]
    fn test_join_generated_for_filter_dimension_in_derived_metric() {
        let mut store = MockStore::new();

        // Base simple metric whose metric_filter references `user__is_internal`
        // — a dimension on a separate model not in group_by.
        let base_filter = r#"{"where_filters": [
            {"where_sql_template": "{{ Dimension('user__is_internal') }} = False"}
        ]}"#;
        let base_type_params = r#"{
            "expr": "order_id",
            "metric_aggregation_params": {
                "semantic_model": "fct_orders",
                "agg": "count_distinct",
                "agg_time_dimension": "order_date"
            }
        }"#;
        store.metrics.push(RawMetricRow {
            name: "order_count".into(),
            metric_type: "simple".into(),
            description: String::new(),
            type_params: base_type_params.into(),
            metric_filter: base_filter.into(),
            time_granularity: None,
        });

        // Derived metric wrapping the simple one — forces the CTE code path.
        let derived_type_params = r#"{
            "expr": "order_count",
            "metrics": [{"name": "order_count"}]
        }"#;
        store.metrics.push(RawMetricRow {
            name: "order_count_derived".into(),
            metric_type: "derived".into(),
            description: String::new(),
            type_params: derived_type_params.into(),
            metric_filter: String::new(),
            time_granularity: None,
        });

        // Primary model: fct_orders
        store.models.push(RawModelRow {
            name: "fct_orders".into(),
            node_relation: r#""db"."main"."fct_orders""#.into(),
            primary_entity: "order".into(),
            unique_id: "semantic_model.fct_orders".into(),
            ..Default::default()
        });
        store.entities.push((
            "semantic_model.fct_orders".into(),
            vec![
                RawEntityRow {
                    name: "order".into(),
                    entity_type: "primary".into(),
                    expr: "order_id".into(),
                },
                RawEntityRow {
                    name: "user".into(),
                    entity_type: "foreign".into(),
                    expr: "user_id".into(),
                },
            ],
        ));
        store.dimensions.push((
            "semantic_model.fct_orders".into(),
            vec![RawDimensionRow {
                name: "order_date".into(),
                dimension_type: "time".into(),
                expr: "order_date".into(),
                time_granularity: "day".into(),
                is_partition: false,
            }],
        ));

        // Secondary model: dim_users (holds is_internal)
        store.models.push(RawModelRow {
            name: "dim_users".into(),
            node_relation: r#""db"."main"."dim_users""#.into(),
            primary_entity: "user".into(),
            unique_id: "semantic_model.dim_users".into(),
            ..Default::default()
        });
        store.entities.push((
            "semantic_model.dim_users".into(),
            vec![RawEntityRow {
                name: "user".into(),
                entity_type: "primary".into(),
                expr: "user_id".into(),
            }],
        ));
        store.dimensions.push((
            "semantic_model.dim_users".into(),
            vec![RawDimensionRow {
                name: "is_internal".into(),
                dimension_type: "categorical".into(),
                expr: "is_internal".into(),
                time_granularity: String::new(),
                is_partition: false,
            }],
        ));

        // Join graph: fct_orders → dim_users via user_id
        store.join_graph.extend([
            RawJoinGraphRow {
                model_name: "fct_orders".into(),
                entity_name: "order".into(),
                entity_type: "primary".into(),
                expr: "order_id".into(),
            },
            RawJoinGraphRow {
                model_name: "fct_orders".into(),
                entity_name: "user".into(),
                entity_type: "foreign".into(),
                expr: "user_id".into(),
            },
            RawJoinGraphRow {
                model_name: "dim_users".into(),
                entity_name: "user".into(),
                entity_type: "primary".into(),
                expr: "user_id".into(),
            },
        ]);

        let spec = SemanticQuerySpec {
            metrics: vec!["order_count_derived".into()],
            // No group_by referencing the user model — the join must come from
            // the metric filter dimension alone.
            group_by: vec![],
            where_filters: vec![],
            order_by: vec![],
            limit: None,
            time_constraint: None,
            apply_group_by: true,
        };

        let sql = compile(&mut store, &spec, Dialect::DuckDB).unwrap();
        // dim_users must be joined so the alias used in the WHERE clause is valid.
        assert!(
            sql.contains("LEFT JOIN"),
            "expected a LEFT JOIN for the filter dimension:\n{sql}"
        );
        assert!(
            sql.contains("dim_users"),
            "dim_users must appear in a JOIN clause:\n{sql}"
        );
        assert!(
            sql.contains("is_internal"),
            "is_internal column must appear in WHERE:\n{sql}"
        );
    }

    // ── Bug fix: ambiguous column in multi-metric simple join ─────────────────

    /// Regression: when two simple metrics come from different models, the
    /// secondary metric's measure `expr` is passed to `qualify_measure_expr`.
    /// That function qualifies simple identifiers (e.g. `user_id` → `f1.user_id`)
    /// but bails out on any expr that contains spaces or parentheses, returning
    /// it verbatim.  For the `account_signups` metric the expr is
    /// `case when account_id is not null then 1 else 0 end`.
    ///
    /// When the two models are joined, both the primary model and the secondary
    /// model's join key are named `account_id`, making the bare reference
    /// ambiguous.  Snowflake rejects the query with:
    ///   "SQL compilation error: ambiguous column name 'ACCOUNT_ID'"
    ///
    /// The fix must qualify every bare column reference inside complex exprs
    /// with the secondary model's alias (e.g. `a.account_id`).
    #[test]
    fn test_multi_metric_simple_join_qualifies_complex_measure_expr() {
        let mut store = MockStore::new();

        // Metric 1: count_distinct of user_id, on fct_user_activities.
        let user_count_params = r#"{
            "expr": "user_id",
            "metric_aggregation_params": {
                "semantic_model": "fct_user_activities",
                "agg": "count_distinct",
                "agg_time_dimension": "date_day"
            }
        }"#;
        store.metrics.push(RawMetricRow {
            name: "user_count".into(),
            metric_type: "simple".into(),
            description: String::new(),
            type_params: user_count_params.into(),
            metric_filter: String::new(),
            time_granularity: None,
        });

        // Metric 2: sum of a CASE expression, on account_signups.
        // The CASE expr references `account_id` which is also the join key on
        // the primary model side — making it ambiguous without a table alias.
        let signup_count_params = r#"{
            "expr": "case when account_id is not null then 1 else 0 end",
            "metric_aggregation_params": {
                "semantic_model": "account_signups",
                "agg": "sum",
                "agg_time_dimension": "created_at"
            }
        }"#;
        store.metrics.push(RawMetricRow {
            name: "signup_count".into(),
            metric_type: "simple".into(),
            description: String::new(),
            type_params: signup_count_params.into(),
            metric_filter: String::new(),
            time_granularity: None,
        });

        // Primary model: fct_user_activities
        store.models.push(RawModelRow {
            name: "fct_user_activities".into(),
            node_relation: r#""db"."main"."fct_user_activities""#.into(),
            primary_entity: "activity".into(),
            unique_id: "semantic_model.fct_user_activities".into(),
            ..Default::default()
        });
        store.entities.push((
            "semantic_model.fct_user_activities".into(),
            vec![
                RawEntityRow {
                    name: "activity".into(),
                    entity_type: "primary".into(),
                    expr: "activity_id".into(),
                },
                RawEntityRow {
                    // FK linking to account_signups.
                    name: "account".into(),
                    entity_type: "foreign".into(),
                    expr: "account_id".into(),
                },
            ],
        ));
        store.dimensions.push((
            "semantic_model.fct_user_activities".into(),
            vec![RawDimensionRow {
                name: "date_day".into(),
                dimension_type: "time".into(),
                expr: "date_day".into(),
                time_granularity: "day".into(),
                is_partition: false,
            }],
        ));

        // Secondary model: account_signups
        store.models.push(RawModelRow {
            name: "account_signups".into(),
            node_relation: r#""db"."main"."account_signups""#.into(),
            primary_entity: "account".into(),
            unique_id: "semantic_model.account_signups".into(),
            ..Default::default()
        });
        store.entities.push((
            "semantic_model.account_signups".into(),
            vec![RawEntityRow {
                name: "account".into(),
                entity_type: "primary".into(),
                expr: "account_id".into(),
            }],
        ));
        store.dimensions.push((
            "semantic_model.account_signups".into(),
            vec![RawDimensionRow {
                name: "created_at".into(),
                dimension_type: "time".into(),
                expr: "created_at".into(),
                time_granularity: "day".into(),
                is_partition: false,
            }],
        ));

        // Join graph: fct_user_activities (FK) → account_signups (PK) via account_id.
        store.join_graph.extend([
            RawJoinGraphRow {
                model_name: "fct_user_activities".into(),
                entity_name: "account".into(),
                entity_type: "foreign".into(),
                expr: "account_id".into(),
            },
            RawJoinGraphRow {
                model_name: "account_signups".into(),
                entity_name: "account".into(),
                entity_type: "primary".into(),
                expr: "account_id".into(),
            },
        ]);

        // Because the two metrics come from different semantic models, the router
        // sends this to compile_complex_metrics (CTE path).  Each metric gets its
        // own CTE with its own FROM clause, so column references inside either
        // metric's expression are unambiguous by construction — only one table is
        // in scope per CTE.  No JOIN between the two base tables occurs.

        let spec = SemanticQuerySpec {
            metrics: vec!["user_count".into(), "signup_count".into()],
            group_by: vec![],
            where_filters: vec![],
            order_by: vec![],
            limit: None,
            time_constraint: None,
            apply_group_by: true,
        };

        let sql = compile(&mut store, &spec, Dialect::DuckDB).unwrap();

        // CTE path: each metric in its own subquery scope.
        assert!(sql.contains("WITH"), "expected CTE-based query:\n{sql}");
        // signup_count CTE evaluates the CASE with only account_signups in scope —
        // account_id is unambiguous without any table qualifier.
        assert!(
            sql.contains("case when account_id is not null"),
            "signup_count CTE must contain the CASE expression:\n{sql}"
        );
        // The two base tables must not be joined together.
        assert!(
            !sql.contains("LEFT JOIN"),
            "metrics from different models must not be combined via LEFT JOIN:\n{sql}"
        );
    }

    // ── Bug fix: derived-table wrapping of complex secondary measure exprs ────
    //
    // compile_simple_metrics wraps the JOIN for any secondary model whose metric
    // expression is "complex" (contains spaces or parens — e.g. a CASE expression)
    // inside a derived table that pre-computes the expr as a named column.
    // This makes all column references unambiguous by construction.
    //
    // The compile() router now sends multi-model simple metrics to the CTE path,
    // so this test calls compile_simple_metrics directly to exercise the mechanism.
    #[test]
    fn test_compile_simple_metrics_wraps_complex_secondary_expr_in_derived_table() {
        let primary = ResolvedModel {
            name: "fct_user_activities".into(),
            relation_name: "fct_user_activities".into(),
            alias: "fct_user_activities".into(),
            schema_name: "main".into(),
            database: "db".into(),
            primary_entity: Some("activity".into()),
            entities: vec![
                EntityDef {
                    name: "activity".into(),
                    entity_type: "primary".into(),
                    expr: "activity_id".into(),
                },
                EntityDef {
                    name: "account".into(),
                    entity_type: "foreign".into(),
                    expr: "account_id".into(),
                },
            ],
            dimensions: vec![DimensionDef {
                name: "date_day".into(),
                dimension_type: "time".into(),
                expr: "date_day".into(),
                time_granularity: Some("day".into()),
                is_partition: false,
            }],
            scd_valid_from: None,
            scd_valid_to: None,
        };

        let secondary = ResolvedModel {
            name: "account_signups".into(),
            relation_name: "account_signups".into(),
            alias: "account_signups".into(),
            schema_name: "main".into(),
            database: "db".into(),
            primary_entity: Some("account".into()),
            entities: vec![EntityDef {
                name: "account".into(),
                entity_type: "primary".into(),
                expr: "account_id".into(),
            }],
            dimensions: vec![DimensionDef {
                name: "created_at".into(),
                dimension_type: "time".into(),
                expr: "created_at".into(),
                time_granularity: Some("day".into()),
                is_partition: false,
            }],
            scd_valid_from: None,
            scd_valid_to: None,
        };

        let user_count = ResolvedMetric {
            name: "user_count".into(),
            metric_type: MetricType::Simple,
            description: String::new(),
            agg_params: Some(AggParams {
                semantic_model: "fct_user_activities".into(),
                agg: "count_distinct".into(),
                expr: "user_id".into(),
                agg_time_dimension: Some("date_day".into()),
                non_additive_dimension: None,
                percentile: None,
                use_discrete_percentile: false,
            }),
            metric_filters: vec![],
            derived_expr: None,
            input_metrics: vec![],
            numerator: None,
            denominator: None,
            cumulative_params: None,
            conversion_params: None,
            join_to_timespine: false,
            fill_nulls_with: None,
            time_granularity: None,
        };

        let signup_count = ResolvedMetric {
            name: "signup_count".into(),
            metric_type: MetricType::Simple,
            description: String::new(),
            agg_params: Some(AggParams {
                semantic_model: "account_signups".into(),
                agg: "sum".into(),
                expr: "case when account_id is not null then 1 else 0 end".into(),
                agg_time_dimension: Some("created_at".into()),
                non_additive_dimension: None,
                percentile: None,
                use_discrete_percentile: false,
            }),
            metric_filters: vec![],
            derived_expr: None,
            input_metrics: vec![],
            numerator: None,
            denominator: None,
            cumulative_params: None,
            conversion_params: None,
            join_to_timespine: false,
            fill_nulls_with: None,
            time_granularity: None,
        };

        // account_signups (i=0) → alias "a"; fct_user_activities (i=1) → alias "f1"
        let model_aliases: HashMap<String, (String, &ResolvedModel)> = [
            ("account_signups".to_string(), ("a".to_string(), &secondary)),
            (
                "fct_user_activities".to_string(),
                ("f1".to_string(), &primary),
            ),
        ]
        .into();

        let join_edges = vec![JoinEdge {
            from_model: "fct_user_activities".into(),
            to_model: "account_signups".into(),
            from_expr: "account_id".into(),
            to_expr: "account_id".into(),
            entity_name: "account".into(),
        }];

        let all_metrics: HashMap<String, ResolvedMetric> = [
            ("user_count".to_string(), user_count.clone()),
            ("signup_count".to_string(), signup_count.clone()),
        ]
        .into();

        let spec = SemanticQuerySpec {
            metrics: vec!["user_count".into(), "signup_count".into()],
            group_by: vec![],
            where_filters: vec![],
            order_by: vec![],
            limit: None,
            time_constraint: None,
            apply_group_by: true,
        };

        let sql = compile_simple_metrics(
            &spec,
            &[&user_count, &signup_count],
            &all_metrics,
            &model_aliases,
            &join_edges,
            &[],
            Dialect::DuckDB,
        )
        .unwrap();

        // Complex expr must be pre-computed inside the derived table.
        assert!(
            sql.contains("SUM(a.__mf_signup_count_expr)"),
            "outer SELECT must reference the pre-computed derived column:\n{sql}"
        );
        assert!(
            sql.contains("__mf_signup_count_expr"),
            "derived table must define the named column:\n{sql}"
        );
        assert!(
            !sql.contains("SUM(case when"),
            "CASE expression must not appear directly in the outer aggregation:\n{sql}"
        );
    }

    // ── Bug: {{ Metric(...) }} filter inside a simple metric used by a derived metric ──
    //
    // When a simple metric's `filter` references `{{ Metric('x', group_by=['e']) }}`,
    // `resolve_where_filter` converts it to `__mf_x.e__x` — a reference to a CTE
    // that must be defined and joined before the WHERE clause is evaluated.
    //
    // `compile_simple_metric_cte` (used by the CTE/derived path) previously never
    // called `compile_metric_filter_ctes`, so the `__mf_*` CTE was never emitted and
    // the identifier was left dangling, producing a Snowflake/DuckDB error like:
    //   "unresolved identifier '__mf_opportunity_delta_average_arr'"
    //
    // This test mirrors the `arr_churn` case:
    //   arr_churn (derived)
    //     └─ gross_churn (simple, filter: {{ Metric('delta_arr', ['opportunity']) }} < 0)
    //   delta_arr (simple, the filter metric)
    //
    // The compiled SQL must:
    //   1. Define a `__mf_delta_arr` CTE before the `gross_churn` CTE.
    //   2. Add a `LEFT JOIN __mf_delta_arr` inside the `gross_churn` CTE body.
    //   3. NOT leave `__mf_delta_arr` as a bare unjoined identifier in WHERE.
    #[test]
    fn test_metric_filter_cte_emitted_inside_derived_metric_cte() {
        let mut store = MockStore::new();

        // The filter metric: delta_arr — simple SUM on fct_opportunities.
        store.metrics.push(RawMetricRow {
            name: "delta_arr".into(),
            metric_type: "simple".into(),
            description: String::new(),
            type_params: r#"{
                "expr": "delta_arr",
                "metric_aggregation_params": {
                    "semantic_model": "fct_opportunities",
                    "agg": "sum",
                    "agg_time_dimension": "close_date"
                }
            }"#
            .into(),
            metric_filter: String::new(),
            time_granularity: None,
        });

        // The filtered simple metric: gross_churn — SUM of delta_arr, but only
        // where the per-opportunity delta_arr metric is negative.
        store.metrics.push(RawMetricRow {
            name: "gross_churn".into(),
            metric_type: "simple".into(),
            description: String::new(),
            type_params: r#"{
                "expr": "delta_arr",
                "metric_aggregation_params": {
                    "semantic_model": "fct_opportunities",
                    "agg": "sum",
                    "agg_time_dimension": "close_date"
                }
            }"#
            .into(),
            // This is the filter that triggers the bug: {{ Metric(...) }} reference.
            metric_filter: r#"{"where_filters": [
                {"where_sql_template": "{{ Metric('delta_arr', group_by=['opportunity']) }} < 0"}
            ]}"#
            .into(),
            time_granularity: None,
        });

        // The derived metric: arr_churn — forces compilation through compile_complex_metrics.
        store.metrics.push(RawMetricRow {
            name: "arr_churn".into(),
            metric_type: "derived".into(),
            description: String::new(),
            type_params: r#"{
                "expr": "gross_churn",
                "metrics": [{"name": "gross_churn"}]
            }"#
            .into(),
            metric_filter: String::new(),
            time_granularity: None,
        });

        // Single semantic model: fct_opportunities with entity `opportunity`.
        store.models.push(RawModelRow {
            name: "fct_opportunities".into(),
            node_relation: r#""db"."main"."fct_opportunities""#.into(),
            primary_entity: "opportunity".into(),
            unique_id: "semantic_model.fct_opportunities".into(),
            ..Default::default()
        });
        store.entities.push((
            "semantic_model.fct_opportunities".into(),
            vec![RawEntityRow {
                name: "opportunity".into(),
                entity_type: "primary".into(),
                expr: "opportunity_id".into(),
            }],
        ));
        store.dimensions.push((
            "semantic_model.fct_opportunities".into(),
            vec![RawDimensionRow {
                name: "close_date".into(),
                dimension_type: "time".into(),
                expr: "close_date".into(),
                time_granularity: "day".into(),
                is_partition: false,
            }],
        ));
        store.join_graph.push(RawJoinGraphRow {
            model_name: "fct_opportunities".into(),
            entity_name: "opportunity".into(),
            entity_type: "primary".into(),
            expr: "opportunity_id".into(),
        });

        let spec = SemanticQuerySpec {
            metrics: vec!["arr_churn".into()],
            group_by: vec![],
            where_filters: vec![],
            order_by: vec![],
            limit: Some(5),
            time_constraint: None,
            apply_group_by: true,
        };

        let sql = compile(&mut store, &spec, Dialect::DuckDB).unwrap();

        // The __mf_delta_arr CTE must be defined somewhere in the WITH block.
        assert!(
            sql.contains("__mf_delta_arr"),
            "__mf_delta_arr CTE must be emitted:\n{sql}"
        );
        // It must be defined as a CTE entry, not just referenced.
        assert!(
            sql.contains("__mf_delta_arr AS ("),
            "__mf_delta_arr must appear as a CTE definition:\n{sql}"
        );
        // The gross_churn CTE body must JOIN the filter CTE so the WHERE reference resolves.
        assert!(
            sql.contains("LEFT JOIN __mf_delta_arr"),
            "gross_churn CTE must LEFT JOIN __mf_delta_arr:\n{sql}"
        );
    }

    // ── Bug: granularity-qualified order-by is rejected even when the group-by matches ──
    //
    // `--group-by metric_time__month` stores the time dimension as
    // `GroupBySpec::TimeDimension { name: "metric_time", granularity: "month" }`.
    // The validation builds `group_by_names` from the `name` field only, so it
    // contains `"metric_time"` but not `"metric_time__month"`.  When the user then
    // passes `--order-by metric_time__month` (the same token they typed for group-by),
    // validation fails with "unknown order-by: metric_time__month".
    //
    // Fix (Option A): expand `group_by_names` to also include `{name}__{granularity}`
    // for every TimeDimension group-by, so both the base name and the qualified form
    // are accepted.

    /// Builds a minimal one-metric MockStore over a single model with a time dimension.
    /// Reused by both order-by tests below to avoid duplication.
    fn order_by_store() -> MockStore {
        let mut store = MockStore::new();
        store.metrics.push(RawMetricRow {
            name: "signups".into(),
            metric_type: "simple".into(),
            description: String::new(),
            type_params: r#"{
                "expr": "1",
                "metric_aggregation_params": {
                    "semantic_model": "fct_signups",
                    "agg": "sum",
                    "agg_time_dimension": "created_at"
                }
            }"#
            .into(),
            metric_filter: String::new(),
            time_granularity: None,
        });
        store.models.push(RawModelRow {
            name: "fct_signups".into(),
            node_relation: r#""db"."main"."fct_signups""#.into(),
            primary_entity: "signup".into(),
            unique_id: "semantic_model.fct_signups".into(),
            ..Default::default()
        });
        store.entities.push((
            "semantic_model.fct_signups".into(),
            vec![RawEntityRow {
                name: "signup".into(),
                entity_type: "primary".into(),
                expr: "signup_id".into(),
            }],
        ));
        store.dimensions.push((
            "semantic_model.fct_signups".into(),
            vec![RawDimensionRow {
                name: "created_at".into(),
                dimension_type: "time".into(),
                expr: "created_at".into(),
                time_granularity: "day".into(),
                is_partition: false,
            }],
        ));
        store.join_graph.push(RawJoinGraphRow {
            model_name: "fct_signups".into(),
            entity_name: "signup".into(),
            entity_type: "primary".into(),
            expr: "signup_id".into(),
        });
        store
    }

    /// Positive: `--order-by metric_time__month` must be accepted when the query
    /// already groups by `metric_time__month`.  The compiled SQL must contain an
    /// ORDER BY clause referencing the canonical column name `metric_time`.
    #[test]
    fn test_order_by_granularity_qualified_name_accepted() {
        let mut store = order_by_store();
        let spec = SemanticQuerySpec {
            metrics: vec!["signups".into()],
            group_by: vec![GroupBySpec::TimeDimension {
                name: "metric_time".into(),
                granularity: "month".into(),
                date_part: None,
            }],
            where_filters: vec![],
            order_by: vec![OrderBySpec {
                name: "metric_time__month".into(),
                descending: false,
            }],
            limit: Some(5),
            time_constraint: None,
            apply_group_by: true,
        };

        let sql = compile(&mut store, &spec, Dialect::DuckDB).expect(
            "order-by metric_time__month should be valid when group-by is metric_time__month",
        );

        // Must use the canonical column name, not the granularity-qualified form.
        assert!(
            sql.contains("ORDER BY metric_time ASC"),
            "SQL must contain ORDER BY metric_time ASC (not metric_time__month):\n{sql}"
        );
        assert!(
            !sql.contains("metric_time__month"),
            "granularity qualifier must not appear in emitted SQL:\n{sql}"
        );
    }

    /// Negative: `--order-by metric_time__day` must still be rejected when the query
    /// groups by `metric_time__month` — the granularities don't match, so the day
    /// column does not exist in the output.
    #[test]
    fn test_order_by_mismatched_granularity_rejected() {
        let mut store = order_by_store();
        let spec = SemanticQuerySpec {
            metrics: vec!["signups".into()],
            group_by: vec![GroupBySpec::TimeDimension {
                name: "metric_time".into(),
                granularity: "month".into(),
                date_part: None,
            }],
            where_filters: vec![],
            order_by: vec![OrderBySpec {
                name: "metric_time__day".into(),
                descending: false,
            }],
            limit: Some(5),
            time_constraint: None,
            apply_group_by: true,
        };

        let result = compile(&mut store, &spec, Dialect::DuckDB);
        assert!(
            result.is_err(),
            "order-by metric_time__day should be rejected when group-by is metric_time__month"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown order-by"),
            "error should mention 'unknown order-by':\n{err}"
        );
    }

    #[test]
    fn test_no_duplicate_cte_for_shared_derived_input() {
        let mut store = MockStore::new();

        store.metrics.push(RawMetricRow {
            name: "base".into(),
            metric_type: "simple".into(),
            description: String::new(),
            type_params: r#"{
                "metric_aggregation_params": {
                    "semantic_model": "fct_events",
                    "agg": "count",
                    "expr": "1",
                    "agg_time_dimension": "ds"
                }
            }"#
            .into(),
            metric_filter: String::new(),
            time_granularity: None,
        });

        store.metrics.push(RawMetricRow {
            name: "wrapped".into(),
            metric_type: "derived".into(),
            description: String::new(),
            type_params: r#"{
                "expr": "base_alias",
                "metrics": [{"name": "base", "alias": "base_alias", "offset_window": null, "offset_to_grain": null, "filter": null}]
            }"#
            .into(),
            metric_filter: String::new(),
            time_granularity: None,
        });

        store.metrics.push(RawMetricRow {
            name: "wrapped_last_period".into(),
            metric_type: "derived".into(),
            description: String::new(),
            type_params: r#"{
                "expr": "wrapped_period_alias",
                "metrics": [{"name": "wrapped", "alias": "wrapped_period_alias", "offset_window": "1 day", "offset_to_grain": null, "filter": null}]
            }"#
            .into(),
            metric_filter: String::new(),
            time_granularity: None,
        });

        store.metrics.push(RawMetricRow {
            name: "wrapped_growth".into(),
            metric_type: "derived".into(),
            description: String::new(),
            type_params: r#"{
                "expr": "wrapped - wrapped_last_period",
                "metrics": [
                    {"name": "wrapped",             "alias": null, "offset_window": null, "offset_to_grain": null, "filter": null},
                    {"name": "wrapped_last_period", "alias": null, "offset_window": null, "offset_to_grain": null, "filter": null}
                ]
            }"#
            .into(),
            metric_filter: String::new(),
            time_granularity: None,
        });

        store.models.push(RawModelRow {
            name: "fct_events".into(),
            node_relation: r#"{"relation_name": "\"db\".\"main\".\"fct_events\"", "alias": "fct_events", "schema_name": "main", "database": "db"}"#.into(),
            primary_entity: "event".into(),
            unique_id: "semantic_model.fct_events".into(),
            ..Default::default()
        });
        store.dimensions.push((
            "semantic_model.fct_events".into(),
            vec![RawDimensionRow {
                name: "ds".into(),
                dimension_type: "time".into(),
                expr: "ds".into(),
                time_granularity: "day".into(),
                is_partition: false,
            }],
        ));

        let spec = SemanticQuerySpec {
            metrics: vec!["wrapped_growth".into()],
            group_by: vec![GroupBySpec::TimeDimension {
                name: "metric_time".into(),
                granularity: "day".into(),
                date_part: None,
            }],
            where_filters: vec![],
            order_by: vec![],
            limit: None,
            time_constraint: None,
            apply_group_by: true,
        };

        let sql = compile(&mut store, &spec, Dialect::DuckDB)
            .expect("wrapped_growth should compile without error");

        let cte_defs = sql.matches("wrapped AS (").count();
        assert_eq!(
            cte_defs, 1,
            "CTE 'wrapped' must be defined exactly once\n  SQL:\n{sql}"
        );
    }

    #[test]
    fn test_no_duplicate_cte_when_offset_alias_equals_metric_name() {
        let mut store = MockStore::new();

        store.metrics.push(RawMetricRow {
            name: "base".into(),
            metric_type: "simple".into(),
            description: String::new(),
            type_params: r#"{
                "metric_aggregation_params": {
                    "semantic_model": "fct_events",
                    "agg": "count",
                    "expr": "1",
                    "agg_time_dimension": "ds"
                }
            }"#
            .into(),
            metric_filter: String::new(),
            time_granularity: None,
        });

        store.metrics.push(RawMetricRow {
            name: "base_last_year".into(),
            metric_type: "derived".into(),
            description: String::new(),
            type_params: r#"{
                "expr": "base_last_year",
                "metrics": [{
                    "name": "base",
                    "alias": "base_last_year",
                    "offset_window": "1 year",
                    "offset_to_grain": null,
                    "filter": null
                }]
            }"#
            .into(),
            metric_filter: String::new(),
            time_granularity: None,
        });

        store.metrics.push(RawMetricRow {
            name: "yoy_growth".into(),
            metric_type: "derived".into(),
            description: String::new(),
            type_params: r#"{
                "expr": "base - base_last_year",
                "metrics": [
                    {"name": "base",           "alias": null, "offset_window": null, "offset_to_grain": null, "filter": null},
                    {"name": "base_last_year", "alias": null, "offset_window": null, "offset_to_grain": null, "filter": null}
                ]
            }"#
            .into(),
            metric_filter: String::new(),
            time_granularity: None,
        });

        store.models.push(RawModelRow {
            name: "fct_events".into(),
            node_relation: r#"{"relation_name": "\"db\".\"main\".\"fct_events\"", "alias": "fct_events", "schema_name": "main", "database": "db"}"#.into(),
            primary_entity: "event".into(),
            unique_id: "semantic_model.fct_events".into(),
            ..Default::default()
        });
        store.dimensions.push((
            "semantic_model.fct_events".into(),
            vec![RawDimensionRow {
                name: "ds".into(),
                dimension_type: "time".into(),
                expr: "ds".into(),
                time_granularity: "day".into(),
                is_partition: false,
            }],
        ));

        let spec = SemanticQuerySpec {
            metrics: vec!["yoy_growth".into()],
            group_by: vec![GroupBySpec::TimeDimension {
                name: "metric_time".into(),
                granularity: "month".into(),
                date_part: None,
            }],
            where_filters: vec![],
            order_by: vec![],
            limit: None,
            time_constraint: None,
            apply_group_by: true,
        };

        let sql = compile(&mut store, &spec, Dialect::DuckDB)
            .expect("yoy_growth should compile without error");

        let cte_defs = sql.matches("base_last_year AS (").count();
        assert_eq!(
            cte_defs, 1,
            "CTE 'base_last_year' must be defined exactly once\n  SQL:\n{sql}"
        );
    }

    #[test]
    fn test_offset_window_object_format_is_applied() {
        let mut store = MockStore::new();

        store.metrics.push(RawMetricRow {
            name: "events".into(),
            metric_type: "simple".into(),
            description: String::new(),
            type_params: r#"{
                "metric_aggregation_params": {
                    "semantic_model": "fct_events",
                    "agg": "count",
                    "expr": "1",
                    "agg_time_dimension": "ds"
                }
            }"#
            .into(),
            metric_filter: String::new(),
            time_granularity: None,
        });

        store.metrics.push(RawMetricRow {
            name: "events_last_year".into(),
            metric_type: "derived".into(),
            description: String::new(),
            type_params: r#"{
                "expr": "events_last_year_alias",
                "metrics": [{
                    "name": "events",
                    "alias": "events_last_year_alias",
                    "offset_window": {"count": 1, "granularity": "year"},
                    "offset_to_grain": null,
                    "filter": null
                }]
            }"#
            .into(),
            metric_filter: String::new(),
            time_granularity: None,
        });

        store.models.push(RawModelRow {
            name: "fct_events".into(),
            node_relation: r#"{"relation_name": "\"db\".\"main\".\"fct_events\"", "alias": "fct_events", "schema_name": "main", "database": "db"}"#.into(),
            primary_entity: "event".into(),
            unique_id: "semantic_model.fct_events".into(),
            ..Default::default()
        });
        store.dimensions.push((
            "semantic_model.fct_events".into(),
            vec![RawDimensionRow {
                name: "ds".into(),
                dimension_type: "time".into(),
                expr: "ds".into(),
                time_granularity: "day".into(),
                is_partition: false,
            }],
        ));

        let spec = SemanticQuerySpec {
            metrics: vec!["events_last_year".into()],
            group_by: vec![GroupBySpec::TimeDimension {
                name: "metric_time".into(),
                granularity: "month".into(),
                date_part: None,
            }],
            where_filters: vec![],
            order_by: vec![],
            limit: None,
            time_constraint: None,
            apply_group_by: true,
        };

        let sql = compile(&mut store, &spec, Dialect::DuckDB)
            .expect("events_last_year should compile without error");

        assert!(
            sql.contains("INTERVAL"),
            "SQL must contain an INTERVAL expression for the 1-year offset\n  SQL:\n{sql}"
        );
        assert!(
            sql.contains("INNER JOIN"),
            "offset CTE must use an INNER JOIN to apply the time shift\n  SQL:\n{sql}"
        );
    }

    #[test]
    fn test_filter_on_outer_derived_metric_propagates_to_inputs() {
        let mut store = MockStore::new();

        store.metrics.push(RawMetricRow {
            name: "count_nps".into(),
            metric_type: "simple".into(),
            description: String::new(),
            type_params: r#"{
                "expr": "response_id",
                "metric_aggregation_params": {
                    "semantic_model": "fct_nps",
                    "agg": "count",
                    "agg_time_dimension": "created_at"
                }
            }"#
            .into(),
            metric_filter: String::new(),
            time_granularity: None,
        });

        store.metrics.push(RawMetricRow {
            name: "nps".into(),
            metric_type: "derived".into(),
            description: String::new(),
            type_params: r#"{"expr": "count_nps", "metrics": [{"name": "count_nps"}]}"#.into(),
            metric_filter: String::new(),
            time_granularity: None,
        });

        store.metrics.push(RawMetricRow {
            name: "nps_developer".into(),
            metric_type: "derived".into(),
            description: String::new(),
            type_params: r#"{"expr": "nps", "metrics": [{"name": "nps"}]}"#.into(),
            metric_filter: r#"{"where_filters": [
                {"where_sql_template": "{{ Dimension('nps_survey__account_plan_tier') }} = 'developer'"}
            ]}"#
            .into(),
            time_granularity: None,
        });

        store.models.push(RawModelRow {
            name: "fct_nps".into(),
            node_relation: r#"{"relation_name": "\"db\".\"main\".\"fct_nps\"", "alias": "fct_nps", "schema_name": "main", "database": "db"}"#.into(),
            primary_entity: "nps_survey".into(),
            unique_id: "semantic_model.fct_nps".into(),
            ..Default::default()
        });
        store.entities.push((
            "semantic_model.fct_nps".into(),
            vec![RawEntityRow {
                name: "nps_survey".into(),
                entity_type: "primary".into(),
                expr: "response_id".into(),
            }],
        ));
        store.dimensions.push((
            "semantic_model.fct_nps".into(),
            vec![
                RawDimensionRow {
                    name: "account_plan_tier".into(),
                    dimension_type: "categorical".into(),
                    expr: "account_plan_tier".into(),
                    time_granularity: String::new(),
                    is_partition: false,
                },
                RawDimensionRow {
                    name: "created_at".into(),
                    dimension_type: "time".into(),
                    expr: "created_at".into(),
                    time_granularity: "day".into(),
                    is_partition: false,
                },
            ],
        ));
        store.join_graph.push(RawJoinGraphRow {
            model_name: "fct_nps".into(),
            entity_name: "nps_survey".into(),
            entity_type: "primary".into(),
            expr: "response_id".into(),
        });

        let spec = SemanticQuerySpec {
            metrics: vec!["nps_developer".into()],
            group_by: vec![],
            where_filters: vec![],
            order_by: vec![],
            limit: None,
            time_constraint: None,
            apply_group_by: true,
        };

        let sql = compile(&mut store, &spec, Dialect::DuckDB).unwrap();

        assert!(
            sql.contains("account_plan_tier"),
            "filter must propagate — account_plan_tier missing:\n{sql}"
        );
        assert!(
            sql.contains("'developer'"),
            "= 'developer' must appear in the generated SQL:\n{sql}"
        );
    }

    #[test]
    fn test_two_derived_metrics_with_different_filters_get_independent_ctes() {
        let mut store = MockStore::new();

        store.metrics.push(RawMetricRow {
            name: "count_nps".into(),
            metric_type: "simple".into(),
            description: String::new(),
            type_params: r#"{
                "expr": "response_id",
                "metric_aggregation_params": {
                    "semantic_model": "fct_nps",
                    "agg": "count",
                    "agg_time_dimension": "created_at"
                }
            }"#
            .into(),
            metric_filter: String::new(),
            time_granularity: None,
        });
        store.metrics.push(RawMetricRow {
            name: "nps".into(),
            metric_type: "derived".into(),
            description: String::new(),
            type_params: r#"{"expr": "count_nps", "metrics": [{"name": "count_nps"}]}"#.into(),
            metric_filter: String::new(),
            time_granularity: None,
        });
        store.metrics.push(RawMetricRow {
            name: "nps_developer".into(),
            metric_type: "derived".into(),
            description: String::new(),
            type_params: r#"{"expr": "nps", "metrics": [{"name": "nps"}]}"#.into(),
            metric_filter: r#"{"where_filters": [
                {"where_sql_template": "{{ Dimension('nps_survey__account_plan_tier') }} = 'developer'"}
            ]}"#
            .into(),
            time_granularity: None,
        });
        store.metrics.push(RawMetricRow {
            name: "nps_enterprise".into(),
            metric_type: "derived".into(),
            description: String::new(),
            type_params: r#"{"expr": "nps", "metrics": [{"name": "nps"}]}"#.into(),
            metric_filter: r#"{"where_filters": [
                {"where_sql_template": "{{ Dimension('nps_survey__account_plan_tier') }} = 'enterprise'"}
            ]}"#
            .into(),
            time_granularity: None,
        });

        store.models.push(RawModelRow {
            name: "fct_nps".into(),
            node_relation: r#"{"relation_name": "\"db\".\"main\".\"fct_nps\"", "alias": "fct_nps", "schema_name": "main", "database": "db"}"#.into(),
            primary_entity: "nps_survey".into(),
            unique_id: "semantic_model.fct_nps".into(),
            ..Default::default()
        });
        store.entities.push((
            "semantic_model.fct_nps".into(),
            vec![RawEntityRow {
                name: "nps_survey".into(),
                entity_type: "primary".into(),
                expr: "response_id".into(),
            }],
        ));
        store.dimensions.push((
            "semantic_model.fct_nps".into(),
            vec![
                RawDimensionRow {
                    name: "account_plan_tier".into(),
                    dimension_type: "categorical".into(),
                    expr: "account_plan_tier".into(),
                    time_granularity: String::new(),
                    is_partition: false,
                },
                RawDimensionRow {
                    name: "created_at".into(),
                    dimension_type: "time".into(),
                    expr: "created_at".into(),
                    time_granularity: "day".into(),
                    is_partition: false,
                },
            ],
        ));
        store.join_graph.push(RawJoinGraphRow {
            model_name: "fct_nps".into(),
            entity_name: "nps_survey".into(),
            entity_type: "primary".into(),
            expr: "response_id".into(),
        });

        let spec = SemanticQuerySpec {
            metrics: vec!["nps_developer".into(), "nps_enterprise".into()],
            group_by: vec![],
            where_filters: vec![],
            order_by: vec![],
            limit: None,
            time_constraint: None,
            apply_group_by: true,
        };

        let sql = compile(&mut store, &spec, Dialect::DuckDB).unwrap();

        assert!(
            sql.contains("'developer'"),
            "developer filter must appear in SQL:\n{sql}"
        );
        assert!(
            sql.contains("'enterprise'"),
            "enterprise filter must appear — base CTEs are being shared:\n{sql}"
        );
    }

    // ── Bug: new-style cumulative metrics store input in cumulative_type_params.metric ──
    //
    // dbt 1.9+ changed the manifest format for cumulative metrics: the input metric
    // is stored inside type_params.cumulative_type_params.metric (an object) instead
    // of in the top-level type_params.metrics array (which is now always []).
    //
    //   type_params:
    //     metrics: []                          ← always empty for cumulative
    //     cumulative_type_params:
    //       window: {count: 7, granularity: day}
    //       metric: {name: "base_daily"}       ← input lives here
    //
    // The parser only read type_params.metrics, so input_metrics was always empty,
    // causing compile_cumulative_metric_cte to fail with
    // "cumulative metric X has no aggregation params".
    //
    // The fix is to also pull from cumulative_type_params.metric when metrics is empty.
    #[test]
    fn test_cumulative_metric_with_new_style_input_metric() {
        let mut store = MockStore::new();

        // The base simple metric that provides the aggregation.
        store.metrics.push(RawMetricRow {
            name: "base_daily".into(),
            metric_type: "simple".into(),
            description: String::new(),
            type_params: r#"{
                "metric_aggregation_params": {
                    "semantic_model": "fct_events",
                    "agg": "count",
                    "expr": "1",
                    "agg_time_dimension": "ds"
                }
            }"#
            .into(),
            metric_filter: String::new(),
            time_granularity: None,
        });

        // New-style cumulative: metrics array is empty, input lives in
        // cumulative_type_params.metric — the exact shape dbt 1.9+ emits.
        store.metrics.push(RawMetricRow {
            name: "base_l7d".into(),
            metric_type: "cumulative".into(),
            description: String::new(),
            type_params: r#"{
                "metrics": [],
                "metric_aggregation_params": null,
                "cumulative_type_params": {
                    "window": {"count": 7, "granularity": "day"},
                    "grain_to_date": null,
                    "period_agg": "first",
                    "metric": {
                        "name": "base_daily",
                        "filter": null,
                        "alias": null,
                        "offset_window": null,
                        "offset_to_grain": null
                    }
                }
            }"#
            .into(),
            metric_filter: String::new(),
            time_granularity: None,
        });

        store.models.push(RawModelRow {
            name: "fct_events".into(),
            node_relation: r#"{"relation_name": "\"db\".\"main\".\"fct_events\"", "alias": "fct_events", "schema_name": "main", "database": "db"}"#.into(),
            primary_entity: "event".into(),
            unique_id: "semantic_model.fct_events".into(),
            ..Default::default()
        });
        store.dimensions.push((
            "semantic_model.fct_events".into(),
            vec![RawDimensionRow {
                name: "ds".into(),
                dimension_type: "time".into(),
                expr: "ds".into(),
                time_granularity: "day".into(),
                is_partition: false,
            }],
        ));

        let spec = SemanticQuerySpec {
            metrics: vec!["base_l7d".into()],
            group_by: vec![GroupBySpec::TimeDimension {
                name: "metric_time".into(),
                granularity: "day".into(),
                date_part: None,
            }],
            where_filters: vec![],
            order_by: vec![],
            limit: None,
            time_constraint: None,
            apply_group_by: true,
        };

        // Pre-fix: fails with "cumulative metric base_l7d has no aggregation params".
        // Post-fix: compiles and the SQL contains a rolling window condition.
        let sql = compile(&mut store, &spec, Dialect::DuckDB)
            .expect("new-style cumulative metric should compile without error");

        assert!(
            sql.contains("INTERVAL"),
            "rolling-window cumulative SQL must contain an INTERVAL for the 7-day window\n  SQL:\n{sql}"
        );
    }
}
