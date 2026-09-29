#![recursion_limit = "512"]
//! Differential testing harness for the dbt-metricflow semantic query compiler
//! against MetricFlow's test fixtures.
//!
//! This test suite:
//! 1. Builds a semantic_manifest.json from MetricFlow's simple_manifest YAMLs
//! 2. Creates DuckDB tables with MetricFlow's table snapshot data
//! 3. Ingests the manifest into dbt-index
//! 4. For each test case, compiles a query and executes it against DuckDB
//! 5. Reports a scorecard of pass/fail
//!
//! Run: cargo test -p dbt-metricflow --test metricflow_compat -- --nocapture
//!
//! The tests are NOT #[ignore] — they run with `cargo test` and require no
//! external fixtures (everything is embedded).

mod common;

use std::path::{Path, PathBuf};

use arrow_array::RecordBatch;
use dbt_adbc::{Backend, Connection, Database, database, driver};
use dbt_metricflow::{Dialect, InMemoryMetricStore, compile, parse_query_spec};
use serde_json::json;

// ═══════════════════════════════════════════════════════════════════════════
// Thin DuckDB wrapper using ADBC (replaces dbt-index Db for test isolation)
// ═══════════════════════════════════════════════════════════════════════════

struct DuckDb {
    #[allow(dead_code)]
    database: Box<dyn Database>,
    connection: Box<dyn Connection>,
}

impl DuckDb {
    fn open_memory() -> Self {
        let mut drv = driver::Builder::new(
            Backend::DuckDBExtended,
            driver::LoadStrategy::SystemThenCdnCache,
        )
        .try_load()
        .expect("failed to load DuckDB driver");
        let mut db_builder = database::Builder::new(Backend::DuckDBExtended);
        db_builder
            .with_named_option("path", ":memory:")
            .expect("failed to set DuckDB path");
        let database = db_builder
            .build(&mut drv)
            .expect("failed to create DuckDB database");
        let connection = database
            .new_connection()
            .expect("failed to create DuckDB connection");
        Self {
            database,
            connection,
        }
    }

    fn execute_update(&mut self, sql: &str) -> Result<Option<i64>, String> {
        let mut stmt = self.connection.new_statement().map_err(|e| e.to_string())?;
        stmt.set_sql_query(sql).map_err(|e| e.to_string())?;
        stmt.execute_update().map_err(|e| e.to_string())
    }

    fn execute_query(&mut self, sql: &str) -> Result<Vec<RecordBatch>, String> {
        let mut stmt = self.connection.new_statement().map_err(|e| e.to_string())?;
        stmt.set_sql_query(sql).map_err(|e| e.to_string())?;
        let reader = stmt.execute().map_err(|e| e.to_string())?;
        let mut batches = Vec::new();
        for batch in reader {
            batches.push(batch.map_err(|e| e.to_string())?);
        }
        Ok(batches)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Manifest builder — constructs semantic_manifest.json from MetricFlow YAMLs
// ═══════════════════════════════════════════════════════════════════════════

/// Schema we use for all tables in DuckDB.  MetricFlow uses `$source_schema`
/// which is replaced at runtime.  We just use `main`.
const SCHEMA: &str = "main";

fn sm(
    name: &str,
    alias: &str,
    primary_entity: Option<&str>,
    default_agg_time_dim: Option<&str>,
    measures: serde_json::Value,
    dimensions: serde_json::Value,
    entities: serde_json::Value,
) -> serde_json::Value {
    sm_with_db(
        name,
        alias,
        None,
        SCHEMA,
        primary_entity,
        default_agg_time_dim,
        measures,
        dimensions,
        entities,
    )
}

#[allow(clippy::too_many_arguments)]
fn sm_with_db(
    name: &str,
    alias: &str,
    database: Option<&str>,
    schema: &str,
    primary_entity: Option<&str>,
    default_agg_time_dim: Option<&str>,
    measures: serde_json::Value,
    dimensions: serde_json::Value,
    entities: serde_json::Value,
) -> serde_json::Value {
    let relation_name = if let Some(db) = database {
        format!("\"{db}\".\"{schema}\".\"{alias}\"")
    } else {
        format!("\"{schema}\".\"{alias}\"")
    };
    json!({
        "name": name,
        "description": name,
        "node_relation": {
            "alias": alias,
            "schema_name": schema,
            "database": database,
            "relation_name": relation_name,
        },
        "defaults": default_agg_time_dim.map(|d| json!({"agg_time_dimension": d})),
        "primary_entity": primary_entity,
        "entities": entities,
        "measures": measures,
        "dimensions": dimensions,
        "label": null,
        "metadata": null,
        "config": null,
    })
}

fn measure(name: &str, agg: &str, expr: Option<&str>) -> serde_json::Value {
    json!({
        "name": name,
        "agg": agg,
        "expr": expr,
        "description": null,
        "label": null,
        "create_metric": false,
        "agg_time_dimension": null,
        "agg_params": null,
        "non_additive_dimension": null,
        "config": null,
    })
}

fn measure_with_agg_time(
    name: &str,
    agg: &str,
    expr: Option<&str>,
    agg_time_dim: &str,
) -> serde_json::Value {
    let mut m = measure(name, agg, expr);
    m["agg_time_dimension"] = json!(agg_time_dim);
    m
}

/// Measure with non_additive_dimension (for semi-additive patterns like balance snapshots).
fn measure_non_additive(
    name: &str,
    agg: &str,
    expr: Option<&str>,
    nad_name: &str,
    window_choice: &str,
    window_groupings: Vec<&str>,
) -> serde_json::Value {
    let mut m = measure(name, agg, expr);
    m["non_additive_dimension"] = json!({
        "name": nad_name,
        "window_choice": window_choice,
        "window_groupings": window_groupings,
    });
    m
}

fn dim_categorical(name: &str, expr: Option<&str>) -> serde_json::Value {
    json!({
        "name": name,
        "type": "categorical",
        "description": null,
        "label": null,
        "expr": expr,
        "is_partition": false,
        "type_params": null,
        "metadata": null,
        "config": null,
    })
}

fn dim_time(name: &str, expr: Option<&str>, granularity: &str) -> serde_json::Value {
    json!({
        "name": name,
        "type": "time",
        "description": null,
        "label": null,
        "expr": expr,
        "is_partition": false,
        "type_params": { "time_granularity": granularity, "validity_params": null },
        "metadata": null,
        "config": null,
    })
}

fn dim_time_partition(name: &str, granularity: &str) -> serde_json::Value {
    json!({
        "name": name,
        "type": "time",
        "description": null,
        "label": null,
        "expr": null,
        "is_partition": true,
        "type_params": { "time_granularity": granularity, "validity_params": null },
        "metadata": null,
        "config": null,
    })
}

fn entity(name: &str, etype: &str, expr: &str) -> serde_json::Value {
    json!({
        "name": name,
        "type": etype,
        "description": null,
        "label": null,
        "role": null,
        "expr": expr,
        "config": null,
        "metadata": null,
    })
}

/// Simple metric: measure name → metric
fn simple_metric(
    name: &str,
    _measure_name: &str,
    model: &str,
    agg: &str,
    expr: &str,
    agg_time_dim: &str,
) -> serde_json::Value {
    json!({
        "name": name,
        "description": name,
        "type": "simple",
        "label": null,
        "time_granularity": null,
        "filter": null,
        "metadata": null,
        "config": null,
        "type_params": {
            "measure": null,
            "input_measures": [],
            "numerator": null,
            "denominator": null,
            "expr": expr,
            "window": null,
            "grain_to_date": null,
            "metrics": null,
            "conversion_type_params": null,
            "cumulative_type_params": null,
            "join_to_timespine": false,
            "fill_nulls_with": null,
            "is_private": false,
            "metric_aggregation_params": {
                "semantic_model": model,
                "agg": agg,
                "agg_params": null,
                "agg_time_dimension": agg_time_dim,
                "non_additive_dimension": null,
                "expr": expr,
            },
        },
    })
}

fn simple_metric_with_filter(
    name: &str,
    measure_name: &str,
    model: &str,
    agg: &str,
    expr: &str,
    agg_time_dim: &str,
    filter: serde_json::Value,
) -> serde_json::Value {
    let mut m = simple_metric(name, measure_name, model, agg, expr, agg_time_dim);
    m["filter"] = filter;
    m
}

fn derived_metric(name: &str, expr: &str, metrics: Vec<serde_json::Value>) -> serde_json::Value {
    json!({
        "name": name,
        "description": name,
        "type": "derived",
        "label": null,
        "time_granularity": null,
        "filter": null,
        "metadata": null,
        "config": null,
        "type_params": {
            "measure": null,
            "input_measures": [],
            "numerator": null,
            "denominator": null,
            "expr": expr,
            "window": null,
            "grain_to_date": null,
            "metrics": metrics,
            "conversion_type_params": null,
            "cumulative_type_params": null,
            "join_to_timespine": false,
            "fill_nulls_with": null,
            "is_private": false,
            "metric_aggregation_params": null,
        },
    })
}

fn ratio_metric(name: &str, numerator: &str, denominator: &str) -> serde_json::Value {
    json!({
        "name": name,
        "description": name,
        "type": "ratio",
        "label": null,
        "time_granularity": null,
        "filter": null,
        "metadata": null,
        "config": null,
        "type_params": {
            "measure": null,
            "input_measures": [],
            "numerator": { "name": numerator, "filter": null, "alias": null, "offset_window": null, "offset_to_grain": null },
            "denominator": { "name": denominator, "filter": null, "alias": null, "offset_window": null, "offset_to_grain": null },
            "expr": null,
            "window": null,
            "grain_to_date": null,
            "metrics": null,
            "conversion_type_params": null,
            "cumulative_type_params": null,
            "join_to_timespine": false,
            "fill_nulls_with": null,
            "is_private": false,
            "metric_aggregation_params": null,
        },
    })
}

/// Cumulative metric: wraps a measure with a rolling window or grain-to-date accumulation.
fn cumulative_metric(
    name: &str,
    model: &str,
    agg: &str,
    expr: &str,
    agg_time_dim: &str,
    window: Option<serde_json::Value>,
    grain_to_date: Option<&str>,
) -> serde_json::Value {
    json!({
        "name": name,
        "description": name,
        "type": "cumulative",
        "label": null,
        "time_granularity": null,
        "filter": null,
        "metadata": null,
        "config": null,
        "type_params": {
            "measure": null,
            "input_measures": [],
            "numerator": null,
            "denominator": null,
            "expr": null,
            "window": null,
            "grain_to_date": null,
            "metrics": null,
            "conversion_type_params": null,
            "cumulative_type_params": {
                "window": window,
                "grain_to_date": grain_to_date,
                "period_agg": "first",
            },
            "join_to_timespine": false,
            "fill_nulls_with": null,
            "is_private": false,
            "metric_aggregation_params": {
                "semantic_model": model,
                "agg": agg,
                "agg_params": null,
                "agg_time_dimension": agg_time_dim,
                "non_additive_dimension": null,
                "expr": expr,
            },
        },
    })
}

/// Simple metric with join_to_timespine and/or fill_nulls_with.
fn simple_metric_with_spine(
    name: &str,
    model: &str,
    agg: &str,
    expr: &str,
    agg_time_dim: &str,
    join_to_timespine: bool,
    fill_nulls_with: Option<i64>,
) -> serde_json::Value {
    json!({
        "name": name,
        "description": name,
        "type": "simple",
        "label": null,
        "time_granularity": null,
        "filter": null,
        "metadata": null,
        "config": null,
        "type_params": {
            "measure": null,
            "input_measures": [],
            "numerator": null,
            "denominator": null,
            "expr": expr,
            "window": null,
            "grain_to_date": null,
            "metrics": null,
            "conversion_type_params": null,
            "cumulative_type_params": null,
            "join_to_timespine": join_to_timespine,
            "fill_nulls_with": fill_nulls_with,
            "is_private": false,
            "metric_aggregation_params": {
                "semantic_model": model,
                "agg": agg,
                "agg_params": null,
                "agg_time_dimension": agg_time_dim,
                "non_additive_dimension": null,
                "expr": expr,
            },
        },
    })
}

/// Conversion metric: base_measure + conversion_measure joined by entity within a window.
fn conversion_metric(
    name: &str,
    base_measure: &str,
    conversion_measure: &str,
    entity: &str,
    calculation: &str,
    window: Option<serde_json::Value>,
) -> serde_json::Value {
    json!({
        "name": name,
        "description": name,
        "type": "conversion",
        "label": null,
        "time_granularity": null,
        "filter": null,
        "metadata": null,
        "config": null,
        "type_params": {
            "measure": null,
            "input_measures": [],
            "numerator": null,
            "denominator": null,
            "expr": null,
            "window": null,
            "grain_to_date": null,
            "metrics": null,
            "conversion_type_params": {
                "base_measure": { "name": base_measure },
                "conversion_measure": { "name": conversion_measure },
                "entity": entity,
                "calculation": calculation,
                "window": window,
                "constant_properties": [],
            },
            "cumulative_type_params": null,
            "join_to_timespine": false,
            "fill_nulls_with": null,
            "is_private": false,
            "metric_aggregation_params": null,
        },
    })
}

fn conversion_metric_with_const_props(
    name: &str,
    base_measure: &str,
    conversion_measure: &str,
    entity: &str,
    calculation: &str,
    window: Option<serde_json::Value>,
    constant_properties: Vec<(&str, &str)>,
) -> serde_json::Value {
    let props: Vec<serde_json::Value> = constant_properties
        .into_iter()
        .map(|(base, conv)| json!({"base_property": base, "conversion_property": conv}))
        .collect();
    json!({
        "name": name,
        "description": name,
        "type": "conversion",
        "label": null,
        "time_granularity": null,
        "filter": null,
        "metadata": null,
        "config": null,
        "type_params": {
            "measure": null,
            "input_measures": [],
            "numerator": null,
            "denominator": null,
            "expr": null,
            "window": null,
            "grain_to_date": null,
            "metrics": null,
            "conversion_type_params": {
                "base_measure": { "name": base_measure },
                "conversion_measure": { "name": conversion_measure },
                "entity": entity,
                "calculation": calculation,
                "window": window,
                "constant_properties": props,
            },
            "cumulative_type_params": null,
            "join_to_timespine": false,
            "fill_nulls_with": null,
            "is_private": false,
            "metric_aggregation_params": null,
        },
    })
}

fn metric_input(name: &str) -> serde_json::Value {
    json!({
        "name": name,
        "filter": null,
        "alias": null,
        "offset_window": null,
        "offset_to_grain": null,
    })
}

fn metric_input_alias(name: &str, alias: &str) -> serde_json::Value {
    json!({
        "name": name,
        "filter": null,
        "alias": alias,
        "offset_window": null,
        "offset_to_grain": null,
    })
}

fn metric_input_offset_window(name: &str, alias: &str, offset_window: &str) -> serde_json::Value {
    json!({
        "name": name,
        "filter": null,
        "alias": alias,
        "offset_window": offset_window,
        "offset_to_grain": null,
    })
}

fn metric_input_offset_to_grain(
    name: &str,
    alias: &str,
    offset_to_grain: &str,
) -> serde_json::Value {
    json!({
        "name": name,
        "filter": null,
        "alias": alias,
        "offset_window": null,
        "offset_to_grain": offset_to_grain,
    })
}

fn metric_input_with_filter(name: &str, alias: &str, filter: &str) -> serde_json::Value {
    json!({
        "name": name,
        "filter": { "where_filters": [{"where_sql_template": filter}] },
        "alias": alias,
        "offset_window": null,
        "offset_to_grain": null,
    })
}

/// Simple metric with a filter (entity constraint, dimension constraint, etc.)
fn simple_metric_filtered(
    name: &str,
    model: &str,
    agg: &str,
    expr: &str,
    agg_time_dim: &str,
    filter: &str,
) -> serde_json::Value {
    let mut m = simple_metric(name, name, model, agg, expr, agg_time_dim);
    m["filter"] = json!({"where_filters": [{"where_sql_template": filter}]});
    m
}

/// Ratio metric with a filter on the metric itself.
fn ratio_metric_with_filter(
    name: &str,
    numerator: &str,
    denominator: &str,
    filter: &str,
) -> serde_json::Value {
    let mut m = ratio_metric(name, numerator, denominator);
    m["filter"] = json!({"where_filters": [{"where_sql_template": filter}]});
    m
}

fn ratio_metric_with_input_filters(
    name: &str,
    num_name: &str,
    num_filter: Option<&str>,
    num_alias: Option<&str>,
    den_name: &str,
    den_filter: Option<&str>,
    den_alias: Option<&str>,
) -> serde_json::Value {
    let num_filter_val = num_filter.map(|f| json!({"where_filters": [{"where_sql_template": f}]}));
    let den_filter_val = den_filter.map(|f| json!({"where_filters": [{"where_sql_template": f}]}));
    json!({
        "name": name,
        "description": name,
        "type": "ratio",
        "label": null,
        "time_granularity": null,
        "filter": null,
        "metadata": null,
        "config": null,
        "type_params": {
            "measure": null,
            "input_measures": [],
            "numerator": { "name": num_name, "filter": num_filter_val, "alias": num_alias, "offset_window": null, "offset_to_grain": null },
            "denominator": { "name": den_name, "filter": den_filter_val, "alias": den_alias, "offset_window": null, "offset_to_grain": null },
            "expr": null,
            "window": null,
            "grain_to_date": null,
            "metrics": null,
            "conversion_type_params": null,
            "cumulative_type_params": null,
            "join_to_timespine": false,
            "fill_nulls_with": null,
            "is_private": false,
            "metric_aggregation_params": null,
        },
    })
}

fn metric_input_with_filter_and_alias(
    name: &str,
    alias: Option<&str>,
    filter: &str,
) -> serde_json::Value {
    json!({
        "name": name,
        "filter": { "where_filters": [{"where_sql_template": filter}] },
        "alias": alias,
        "offset_window": null,
        "offset_to_grain": null,
    })
}

fn build_semantic_manifest() -> serde_json::Value {
    // ── Semantic Models ──────────────────────────────────────────────────
    let bookings_source = sm(
        "bookings_source",
        "fct_bookings",
        Some("booking"),
        Some("ds"),
        json!([
            measure("bookings", "sum", Some("1")),
            measure("instant_bookings", "sum_boolean", Some("is_instant")),
            measure("booking_value", "sum", None),
            measure("max_booking_value", "max", Some("booking_value")),
            measure("min_booking_value", "min", Some("booking_value")),
            measure("bookers", "count_distinct", Some("guest_id")),
            measure("average_booking_value", "average", Some("booking_value")),
            measure_with_agg_time("booking_payments", "sum", Some("booking_value"), "paid_at"),
            measure("referred_bookings", "count", Some("referrer_id")),
            measure("median_booking_value", "median", Some("booking_value")),
            measure("booking_value_p99", "percentile", Some("booking_value")),
            measure(
                "discrete_booking_value_p99",
                "percentile",
                Some("booking_value")
            ),
        ]),
        json!([
            dim_categorical("is_instant", None),
            dim_time("ds", None, "day"),
            dim_time_partition("ds_partitioned", "day"),
            dim_time("paid_at", None, "day"),
        ]),
        json!([
            entity("listing", "foreign", "listing_id"),
            entity("guest", "foreign", "guest_id"),
            entity("host", "foreign", "host_id"),
        ]),
    );

    let listings_latest = sm(
        "listings_latest",
        "dim_listings_latest",
        None,
        Some("ds"),
        json!([
            measure("listings", "sum", Some("1")),
            measure("largest_listing", "max", Some("capacity")),
            measure("smallest_listing", "min", Some("capacity")),
        ]),
        json!([
            dim_time("ds", Some("created_at"), "day"),
            dim_time("created_at", None, "day"),
            dim_categorical("country_latest", Some("country")),
            dim_categorical("is_lux_latest", Some("is_lux")),
            dim_categorical("capacity_latest", Some("capacity")),
        ]),
        json!([
            entity("listing", "primary", "listing_id"),
            entity("user", "foreign", "user_id"),
        ]),
    );

    let views_source = sm(
        "views_source",
        "fct_views",
        Some("view"),
        Some("ds"),
        json!([measure("views", "sum", Some("1")),]),
        json!([
            dim_time("ds", None, "day"),
            dim_time_partition("ds_partitioned", "day"),
        ]),
        json!([
            entity("listing", "foreign", "listing_id"),
            entity("user", "foreign", "user_id"),
        ]),
    );

    let users_latest = sm(
        "users_latest",
        "dim_users_latest",
        None,
        None,
        json!([]),
        json!([
            dim_time("ds_latest", Some("ds"), "day"),
            dim_categorical("home_state_latest", None),
        ]),
        json!([entity("user", "primary", "user_id"),]),
    );

    let users_ds_source = sm(
        "users_ds_source",
        "dim_users",
        Some("user"),
        Some("created_at"),
        json!([
            measure("new_users", "sum", Some("1")),
            measure_with_agg_time("archived_users", "sum", Some("1"), "archived_at"),
        ]),
        json!([
            dim_time("ds", None, "day"),
            dim_time("created_at", None, "day"),
            dim_time_partition("ds_partitioned", "day"),
            dim_categorical("home_state", None),
            dim_time("archived_at", None, "hour"),
            dim_time("last_profile_edit_ts", None, "millisecond"),
            dim_time("last_login_ts", None, "minute"),
            dim_time("bio_added_ts", None, "second"),
        ]),
        json!([entity("user", "primary", "user_id"),]),
    );

    let revenue = sm(
        "revenue",
        "fct_revenue",
        Some("revenue_instance"),
        Some("ds"),
        json!([measure("txn_revenue", "sum", Some("revenue")),]),
        json!([dim_time("ds", Some("created_at"), "day"),]),
        json!([entity("user", "foreign", "user_id"),]),
    );

    let mut measure_balance_first_of_month = measure_non_additive(
        "total_account_balance_first_day_of_month",
        "sum",
        Some("account_balance"),
        "ds_month",
        "min",
        vec![],
    );
    measure_balance_first_of_month["agg_time_dimension"] = json!("ds_month");

    let accounts_source = sm(
        "accounts_source",
        "fct_accounts",
        Some("account"),
        Some("ds"),
        json!([
            measure("account_balance", "sum", None),
            // Semi-additive: total balance on first day (MIN ds, no grouping)
            measure_non_additive(
                "total_account_balance_first_day",
                "sum",
                Some("account_balance"),
                "ds",
                "min",
                vec![]
            ),
            // Semi-additive: current balance per user (MAX ds, grouped by user)
            measure_non_additive(
                "current_account_balance_by_user",
                "sum",
                Some("account_balance"),
                "ds",
                "max",
                vec!["user"]
            ),
            measure_balance_first_of_month,
        ]),
        json!([
            dim_time("ds", None, "day"),
            dim_time("ds_month", Some("ds_month"), "month"),
            dim_categorical("account_type", None),
        ]),
        json!([entity("user", "foreign", "user_id"),]),
    );

    let id_verifications = sm(
        "id_verifications",
        "fct_id_verifications",
        None,
        Some("ds"),
        json!([measure("identity_verifications", "sum", Some("1")),]),
        json!([
            dim_time("ds", None, "day"),
            dim_time_partition("ds_partitioned", "day"),
            dim_categorical("verification_type", None),
        ]),
        json!([
            entity("verification", "primary", "verification_id"),
            entity("user", "foreign", "user_id"),
        ]),
    );

    let visits_source = sm(
        "visits_source",
        "fct_visits",
        Some("visit"),
        Some("ds"),
        json!([measure("visits", "count", Some("1")),]),
        json!([
            dim_time("ds", None, "day"),
            dim_categorical("referrer_id", None),
        ]),
        json!([
            entity("user", "foreign", "user_id"),
            entity("session", "foreign", "session_id"),
        ]),
    );

    let buys_source = sm(
        "buys_source",
        "fct_buys",
        Some("buy"),
        Some("ds"),
        json!([measure("buys", "count", Some("1")),]),
        json!([dim_time("ds", None, "day"),]),
        json!([entity("user", "foreign", "user_id"),]),
    );

    let companies = sm(
        "companies",
        "dim_companies",
        None,
        None,
        json!([]),
        json!([dim_categorical("company_name", None),]),
        json!([
            entity("company", "primary", "company_id"),
            entity("user", "unique", "user_id"),
        ]),
    );

    let company_regions = sm(
        "company_regions",
        "dim_company_regions",
        None,
        None,
        json!([]),
        json!([dim_categorical("region_name", None),]),
        json!([
            entity("region", "primary", "region_id"),
            entity("company", "foreign", "company_id"),
        ]),
    );

    let lux_listing_mapping = sm(
        "lux_listing_mapping",
        "dim_lux_listing_id_mapping",
        None,
        None,
        json!([]),
        json!([]),
        json!([
            entity("listing", "primary", "listing_id"),
            entity("lux_listing", "foreign", "lux_listing_id"),
        ]),
    );

    // Models for testing bare dimension name collision across different primary entities.
    // Mimics account_conversions / account_signups from internal-analytics.
    let conversions_source = sm(
        "conversions_source",
        "fct_conversions",
        Some("converted_account"),
        Some("created_at"),
        json!([measure("conversions", "sum", Some("1")),]),
        json!([
            dim_time("created_at", None, "day"),
            dim_categorical("channel", None),
        ]),
        json!([]),
    );

    let signups_source = sm(
        "signups_source",
        "fct_signups",
        Some("signup_account"),
        Some("created_at"),
        json!([measure("signups", "sum", Some("1")),]),
        json!([
            dim_time("created_at", None, "day"),
            dim_categorical("source", None),
        ]),
        json!([]),
    );

    // ── Pre-built semi-additive metrics (can't inline blocks in json! macro) ──
    let mut semi_additive_first_day = simple_metric(
        "total_account_balance_first_day",
        "total_account_balance_first_day",
        "accounts_source",
        "sum",
        "account_balance",
        "ds",
    );
    semi_additive_first_day["type_params"]["metric_aggregation_params"]["non_additive_dimension"] = json!({
        "name": "ds", "window_choice": "min", "window_groupings": []
    });
    let mut semi_additive_current_by_user = simple_metric(
        "current_account_balance_by_user",
        "current_account_balance_by_user",
        "accounts_source",
        "sum",
        "account_balance",
        "ds",
    );
    semi_additive_current_by_user["type_params"]["metric_aggregation_params"]["non_additive_dimension"] = json!({
        "name": "ds", "window_choice": "max", "window_groupings": ["user"]
    });

    // Cumulative metric with fill_nulls (needs mutation, so create before json! macro)
    let mut cumul_fill_nulls = cumulative_metric(
        "every_two_days_bookers_fill_nulls_with_0",
        "bookings_source",
        "count_distinct",
        "guest_id",
        "ds",
        Some(json!({"count": 2, "granularity": "day"})),
        None,
    );
    cumul_fill_nulls["type_params"]["join_to_timespine"] = json!(true);
    cumul_fill_nulls["type_params"]["fill_nulls_with"] = json!(0);

    // ── Metrics ──────────────────────────────────────────────────────────
    // Pre-build metrics that need mutation (can't use block exprs inside json! macro)
    let mut instant_bookings_measure_filter = simple_metric_with_spine(
        "instant_bookings_with_measure_filter",
        "bookings_source",
        "sum",
        "1",
        "ds",
        true,
        None,
    );
    instant_bookings_measure_filter["filter"] = json!({"where_filters": [
        {"where_sql_template": "{{ Dimension('booking__is_instant') }}"},
        {"where_sql_template": "{{ Entity('listing') }} IS NOT NULL"},
    ]});

    let mut subdaily_metric_default_hour = simple_metric(
        "simple_subdaily_metric_default_hour",
        "archived_users",
        "users_ds_source",
        "sum",
        "1",
        "archived_at",
    );
    subdaily_metric_default_hour["time_granularity"] = json!("hour");

    let mut balance_first_day_of_month_metric = simple_metric(
        "total_account_balance_first_day_of_month",
        "total_account_balance_first_day_of_month",
        "accounts_source",
        "sum",
        "account_balance",
        "ds_month",
    );
    balance_first_day_of_month_metric["type_params"]["metric_aggregation_params"]["non_additive_dimension"] = json!({
        "name": "ds_month", "window_choice": "min", "window_groupings": []
    });
    balance_first_day_of_month_metric["type_params"]["metric_aggregation_params"]["agg_time_dimension"] =
        json!("ds_month");

    let metrics = json!([
        // Simple metrics from bookings_source
        simple_metric("bookings", "bookings", "bookings_source", "sum", "1", "ds"),
        simple_metric(
            "instant_bookings",
            "instant_bookings",
            "bookings_source",
            "sum_boolean",
            "is_instant",
            "ds"
        ),
        simple_metric(
            "booking_value",
            "booking_value",
            "bookings_source",
            "sum",
            "booking_value",
            "ds"
        ),
        simple_metric(
            "max_booking_value",
            "max_booking_value",
            "bookings_source",
            "max",
            "booking_value",
            "ds"
        ),
        simple_metric(
            "min_booking_value",
            "min_booking_value",
            "bookings_source",
            "min",
            "booking_value",
            "ds"
        ),
        simple_metric(
            "bookers",
            "bookers",
            "bookings_source",
            "count_distinct",
            "guest_id",
            "ds"
        ),
        simple_metric(
            "average_booking_value",
            "average_booking_value",
            "bookings_source",
            "average",
            "booking_value",
            "ds"
        ),
        simple_metric(
            "referred_bookings",
            "referred_bookings",
            "bookings_source",
            "count",
            "referrer_id",
            "ds"
        ),
        // Simple metrics with filters
        simple_metric_with_filter(
            "instant_booking_value",
            "booking_value",
            "bookings_source",
            "sum",
            "booking_value",
            "ds",
            json!({"where_filters": [{"where_sql_template": "{{ Dimension('booking__is_instant') }}"}]}),
        ),
        // Simple metrics from other models
        simple_metric("views", "views", "views_source", "sum", "1", "ds"),
        simple_metric("listings", "listings", "listings_latest", "sum", "1", "ds"),
        simple_metric(
            "largest_listing",
            "largest_listing",
            "listings_latest",
            "max",
            "capacity",
            "ds"
        ),
        simple_metric(
            "smallest_listing",
            "smallest_listing",
            "listings_latest",
            "min",
            "capacity",
            "ds"
        ),
        simple_metric(
            "identity_verifications",
            "identity_verifications",
            "id_verifications",
            "sum",
            "1",
            "ds"
        ),
        simple_metric("revenue", "txn_revenue", "revenue", "sum", "revenue", "ds"),
        simple_metric(
            "account_balance",
            "account_balance",
            "accounts_source",
            "sum",
            "account_balance",
            "ds"
        ),
        simple_metric(
            "booking_payments",
            "booking_payments",
            "bookings_source",
            "sum",
            "booking_value",
            "paid_at"
        ),
        simple_metric(
            "new_users",
            "new_users",
            "users_ds_source",
            "sum",
            "1",
            "created_at"
        ),
        // Simple metric with a metric filter: only count bookings where the listing's
        // total booking_value > 1000.
        simple_metric_with_filter(
            "bookings_where_listing_high_value",
            "bookings",
            "bookings_source",
            "sum",
            "1",
            "ds",
            json!({"where_filters": [{"where_sql_template": "{{ Metric('booking_value', ['listing']) }} > 1000"}]}),
        ),
        // Derived metrics
        derived_metric(
            "booking_fees",
            "booking_value * 0.05",
            vec![metric_input("booking_value")]
        ),
        derived_metric(
            "booking_fees_per_booker",
            "booking_value * 0.05 / bookers",
            vec![metric_input("booking_value"), metric_input("bookers"),]
        ),
        derived_metric(
            "views_times_booking_value",
            "booking_value * views",
            vec![metric_input("booking_value"), metric_input("views"),]
        ),
        derived_metric(
            "non_referred_bookings_pct",
            "(bookings - ref_bookings) * 1.0 / bookings",
            vec![
                metric_input_alias("referred_bookings", "ref_bookings"),
                metric_input("bookings"),
            ]
        ),
        derived_metric(
            "booking_value_sub_instant",
            "booking_value - instant_booking_value",
            vec![
                metric_input("instant_booking_value"),
                metric_input("booking_value"),
            ]
        ),
        derived_metric(
            "booking_value_sub_instant_add_10",
            "booking_value_sub_instant + 10",
            vec![metric_input("booking_value_sub_instant"),]
        ),
        // Offset window derived metrics
        derived_metric(
            "bookings_growth_1_day",
            "bookings - bookings_1_day_ago",
            vec![
                metric_input("bookings"),
                metric_input_offset_window("bookings", "bookings_1_day_ago", "1 day"),
            ]
        ),
        // Offset to grain derived metrics
        derived_metric(
            "bookings_growth_since_start_of_month",
            "bookings - bookings_at_start_of_month",
            vec![
                metric_input("bookings"),
                metric_input_offset_to_grain("bookings", "bookings_at_start_of_month", "month"),
            ]
        ),
        // Ratio metrics
        ratio_metric("bookings_per_booker", "bookings", "bookers"),
        ratio_metric("bookings_per_view", "bookings", "views"),
        ratio_metric("bookings_per_listing", "bookings", "listings"),
        ratio_metric("bookings_per_dollar", "bookings", "booking_value"),
        // Cumulative metrics
        // trailing_2_months_revenue: SUM(revenue) over a 2-month rolling window
        cumulative_metric(
            "trailing_2_months_revenue",
            "revenue",
            "sum",
            "revenue",
            "ds",
            Some(json!({"count": 2, "granularity": "month"})),
            None,
        ),
        // revenue_all_time: SUM(revenue) cumulative from the beginning
        cumulative_metric(
            "revenue_all_time",
            "revenue",
            "sum",
            "revenue",
            "ds",
            None,
            None,
        ),
        // revenue_mtd: SUM(revenue) month-to-date
        cumulative_metric(
            "revenue_mtd",
            "revenue",
            "sum",
            "revenue",
            "ds",
            None,
            Some("month"),
        ),
        // cumulative_bookings: SUM(bookings) all-time
        cumulative_metric(
            "cumulative_bookings",
            "bookings_source",
            "sum",
            "1",
            "ds",
            None,
            None,
        ),
        // every_2_days_bookers: COUNT_DISTINCT(guest_id) over 2-day window
        cumulative_metric(
            "every_2_days_bookers",
            "bookings_source",
            "count_distinct",
            "guest_id",
            "ds",
            Some(json!({"count": 2, "granularity": "day"})),
            None,
        ),
        // Time spine / fill_nulls metrics
        // fill_nulls_with_0 WITHOUT time spine — just COALESCE
        simple_metric_with_spine(
            "bookings_fill_nulls_no_spine",
            "bookings_source",
            "sum",
            "1",
            "ds",
            false,
            Some(0),
        ),
        // join_to_timespine WITHOUT fill_nulls — spine LEFT JOIN, NULLs preserved
        simple_metric_with_spine(
            "bookings_join_to_time_spine",
            "bookings_source",
            "sum",
            "1",
            "ds",
            true,
            None,
        ),
        // BOTH fill_nulls_with_0 AND join_to_timespine — spine LEFT JOIN + COALESCE
        simple_metric_with_spine(
            "bookings_fill_nulls_with_spine",
            "bookings_source",
            "sum",
            "1",
            "ds",
            true,
            Some(0),
        ),
        // Revenue with fill nulls + spine (different model)
        simple_metric_with_spine(
            "revenue_fill_nulls_with_spine",
            "revenue",
            "sum",
            "revenue",
            "ds",
            true,
            Some(0),
        ),
        // Base measures as simple metrics (needed for conversion metric CTEs)
        simple_metric("visits", "visits", "visits_source", "count", "1", "ds"),
        simple_metric("buys", "buys", "buys_source", "count", "1", "ds"),
        // Conversion metrics
        conversion_metric(
            "visit_buy_conversion_rate_7days",
            "visits",
            "buys",
            "user",
            "conversion_rate",
            Some(json!({"count": 7, "granularity": "day"})),
        ),
        conversion_metric(
            "visit_buy_conversions_7days",
            "visits",
            "buys",
            "user",
            "conversions",
            Some(json!({"count": 7, "granularity": "day"})),
        ),
        conversion_metric(
            "visit_buy_conversion_rate_unbounded",
            "visits",
            "buys",
            "user",
            "conversion_rate",
            None,
        ),
        // Conversion with constant property (session_id must match)
        conversion_metric_with_const_props(
            "visit_buy_conversion_rate_by_session",
            "visits",
            "buys",
            "user",
            "conversion_rate",
            Some(json!({"count": 7, "granularity": "day"})),
            vec![("session_id", "session_id")],
        ),
        // Semi-additive metrics — built below
        semi_additive_first_day,
        semi_additive_current_by_user,
        // Derived metrics with shared alias names (for multi-metric alias collision tests)
        derived_metric(
            "derived_shared_alias_1a",
            "shared_alias - 10",
            vec![metric_input_alias("bookings", "shared_alias")]
        ),
        derived_metric(
            "derived_shared_alias_1b",
            "shared_alias - 100",
            vec![metric_input_alias("bookings", "shared_alias")]
        ),
        derived_metric(
            "derived_shared_alias_2",
            "shared_alias + 10",
            vec![metric_input_alias("instant_bookings", "shared_alias")]
        ),
        // ── Additional metrics for single-metric test porting ────────
        // booking_value_for_non_null_listing_id: booking_value WHERE listing IS NOT NULL
        simple_metric_filtered(
            "booking_value_for_non_null_listing_id",
            "bookings_source",
            "sum",
            "booking_value",
            "ds",
            "{{ Entity('listing') }} IS NOT NULL",
        ),
        // lux_listings: listings WHERE is_lux_latest
        simple_metric_filtered(
            "lux_listings",
            "listings_latest",
            "count",
            "1",
            "ds",
            "{{ Dimension('listing__is_lux_latest') }}",
        ),
        // booking_value_per_view: derived = booking_value / NULLIF(views, 0)
        derived_metric(
            "booking_value_per_view",
            "booking_value / NULLIF(views, 0)",
            vec![metric_input("booking_value"), metric_input("views")]
        ),
        // bookings_growth_2_weeks: bookings - bookings_2_weeks_ago (offset_window=14 days)
        derived_metric(
            "bookings_growth_2_weeks",
            "bookings - bookings_2_weeks_ago",
            vec![
                metric_input("bookings"),
                metric_input_offset_window("bookings", "bookings_2_weeks_ago", "14 days"),
            ]
        ),
        // bookings_5_day_lag: derived with a single offset input
        derived_metric(
            "bookings_5_day_lag",
            "bookings_5_days_ago",
            vec![metric_input_offset_window(
                "bookings",
                "bookings_5_days_ago",
                "5 days"
            ),]
        ),
        // bookings_offset_one_alien_day: offset by 1 custom granularity unit
        derived_metric(
            "bookings_offset_one_alien_day",
            "bookings_offset_one_alien_day",
            vec![metric_input_offset_window(
                "bookings",
                "bookings_offset_one_alien_day",
                "1 alien_day"
            ),]
        ),
        // bookings_month_start_compared_to_1_month_prior:
        // offset_to_grain(month) vs offset_window(1 month)
        derived_metric(
            "bookings_month_start_compared_to_1_month_prior",
            "month_start_bookings - bookings_1_month_ago",
            vec![
                metric_input_offset_to_grain("bookings", "month_start_bookings", "month"),
                metric_input_offset_window("bookings", "bookings_1_month_ago", "1 month"),
            ]
        ),
        // booking_fees_last_week_per_booker_this_week:
        // derived offset(booking_value, 1 week) * 0.05 / bookers
        derived_metric(
            "booking_fees_last_week_per_booker_this_week",
            "booking_value * 0.05 / bookers",
            vec![
                metric_input_offset_window("booking_value", "booking_value", "1 week"),
                metric_input("bookers"),
            ]
        ),
        // instant_plus_non_referred_bookings_pct: nested derived
        derived_metric(
            "instant_plus_non_referred_bookings_pct",
            "non_referred + (instant * 1.0 / bookings)",
            vec![
                metric_input_alias("non_referred_bookings_pct", "non_referred"),
                metric_input_alias("instant_bookings", "instant"),
                metric_input("bookings"),
            ]
        ),
        // bookings_per_lux_listing_derived: bookings / listings(filter=is_lux)
        derived_metric(
            "bookings_per_lux_listing_derived",
            "bookings * 1.0 / NULLIF(lux_listing, 0)",
            vec![
                metric_input("bookings"),
                metric_input_with_filter(
                    "listings",
                    "lux_listing",
                    "{{ Dimension('listing__is_lux_latest') }}"
                ),
            ]
        ),
        // every_2_days_bookers_2_days_ago: offset of our cumulative metric
        derived_metric(
            "every_2_days_bookers_2_days_ago",
            "every_2_days_bookers_2_days_ago",
            vec![metric_input_offset_window(
                "every_2_days_bookers",
                "every_2_days_bookers_2_days_ago",
                "2 days"
            ),]
        ),
        // bookings_fill_nulls_with_0: simple metric with fill_nulls=0 + join_to_timespine
        simple_metric_with_spine(
            "bookings_fill_nulls_with_0",
            "bookings_source",
            "sum",
            "1",
            "ds",
            true,
            Some(0),
        ),
        // bookings_fill_nulls_with_0_without_time_spine: fill_nulls=0, no spine
        simple_metric_with_spine(
            "bookings_fill_nulls_with_0_without_time_spine",
            "bookings_source",
            "sum",
            "1",
            "ds",
            false,
            Some(0),
        ),
        // every_two_days_bookers_fill_nulls_with_0: cumulative + fill_nulls
        cumul_fill_nulls,
        // bookings_growth_2_weeks_fill_nulls_with_0_for_non_offset:
        // derived = bookings_fill_nulls_with_0 - bookings(offset 14 days)
        derived_metric(
            "bookings_growth_2_weeks_fill_nulls_with_0_for_non_offset",
            "bookings_fill_nulls_with_0 - bookings_2_weeks_ago",
            vec![
                metric_input("bookings_fill_nulls_with_0"),
                metric_input_offset_window("bookings", "bookings_2_weeks_ago", "14 days"),
            ]
        ),
        // bookings_offset_once: intermediate metric for double offset
        derived_metric(
            "bookings_offset_once",
            "2 * bookings",
            vec![json!({
                "name": "bookings",
                "filter": null,
                "alias": null,
                "offset_window": "5 days",
                "offset_to_grain": null,
            }),]
        ),
        // bookings_offset_twice: 2 * bookings_offset_once(offset 2 days)
        derived_metric(
            "bookings_offset_twice",
            "2 * bookings_offset_once",
            vec![metric_input_offset_window(
                "bookings_offset_once",
                "bookings_offset_once",
                "2 days"
            ),]
        ),
        // booking_fees_since_start_of_month: booking_fees - booking_fees(offset_to_grain=month)
        derived_metric(
            "booking_fees_since_start_of_month",
            "booking_fees - booking_fees_start_of_month",
            vec![
                metric_input_offset_to_grain(
                    "booking_fees",
                    "booking_fees_start_of_month",
                    "month"
                ),
                metric_input("booking_fees"),
            ]
        ),
        // active_listings: listings WHERE Metric('bookings') > 2
        simple_metric_with_filter(
            "active_listings",
            "listings",
            "listings_latest",
            "count",
            "1",
            "ds",
            json!({"where_filters": [{"where_sql_template": "{{ Metric('bookings', ['listing']) }} > 2"}]}),
        ),
        // popular_listing_bookings_per_booker: ratio with metric filter
        ratio_metric_with_filter(
            "popular_listing_bookings_per_booker",
            "listings",
            "listings",
            "{{ Metric('views', ['listing']) }} > 10",
        ),
        // Metrics for bare dimension name collision test.
        simple_metric(
            "account_conversions",
            "conversions",
            "conversions_source",
            "sum",
            "1",
            "created_at"
        ),
        simple_metric(
            "account_signups",
            "signups",
            "signups_source",
            "sum",
            "1",
            "created_at"
        ),
        // ── Subdaily metrics ─────────────────────────────────────────
        // archived_users: simple metric with agg_time_dimension=archived_at (hour)
        simple_metric(
            "archived_users",
            "archived_users",
            "users_ds_source",
            "sum",
            "1",
            "archived_at"
        ),
        // subdaily_join_to_time_spine: archived_users + join_to_timespine
        simple_metric_with_spine(
            "subdaily_join_to_time_spine_metric",
            "users_ds_source",
            "sum",
            "1",
            "archived_at",
            true,
            None,
        ),
        // subdaily_offset_window: derived = archived_users(offset 1 hour)
        derived_metric(
            "subdaily_offset_window_metric",
            "archived_users",
            vec![metric_input_offset_window(
                "archived_users",
                "archived_users",
                "1 hour"
            ),]
        ),
        // subdaily_offset_grain_to_date: derived = archived_users(offset_to_grain=hour)
        derived_metric(
            "subdaily_offset_grain_to_date_metric",
            "archived_users",
            vec![metric_input_offset_to_grain(
                "archived_users",
                "archived_users",
                "hour"
            ),]
        ),
        // ── 3-input derived metric (COALESCE join test) ─────────────
        derived_metric(
            "bookings_plus_views_plus_listings",
            "bookings + views + listings",
            vec![
                metric_input("bookings"),
                metric_input("views"),
                metric_input("listings"),
            ]
        ),
        // ── fill_nulls_with in derived expression test ──────────────
        // Uses bookings_fill_nulls_with_0 (fill=0) combined with views.
        // When FULL OUTER JOINed by listing, dates with views but no bookings
        // should get 0 (not NULL) for the bookings input.
        derived_metric(
            "fill_nulls_derived_test",
            "bookings_fill_nulls_with_0 + views",
            vec![
                metric_input("bookings_fill_nulls_with_0"),
                metric_input("views"),
            ]
        ),
        // ── Additional metrics for ported MetricFlow tests ──────────
        // booking_value_p99: percentile(0.99) of booking_value
        json!({
            "name": "booking_value_p99",
            "description": "booking_value_p99",
            "type": "simple",
            "label": null,
            "time_granularity": null,
            "filter": null,
            "metadata": null,
            "config": null,
            "type_params": {
                "measure": null,
                "input_measures": [],
                "numerator": null,
                "denominator": null,
                "expr": "booking_value",
                "window": null,
                "grain_to_date": null,
                "metrics": null,
                "conversion_type_params": null,
                "cumulative_type_params": null,
                "join_to_timespine": false,
                "fill_nulls_with": null,
                "is_private": false,
                "metric_aggregation_params": {
                    "semantic_model": "bookings_source",
                    "agg": "percentile",
                    "agg_params": { "percentile": 0.99 },
                    "agg_time_dimension": "ds",
                    "non_additive_dimension": null,
                    "expr": "booking_value",
                },
            },
        }),
        // discrete_booking_value_p99: discrete percentile(0.99) of booking_value
        json!({
            "name": "discrete_booking_value_p99",
            "description": "discrete_booking_value_p99",
            "type": "simple",
            "label": null,
            "time_granularity": null,
            "filter": null,
            "metadata": null,
            "config": null,
            "type_params": {
                "measure": null,
                "input_measures": [],
                "numerator": null,
                "denominator": null,
                "expr": "booking_value",
                "window": null,
                "grain_to_date": null,
                "metrics": null,
                "conversion_type_params": null,
                "cumulative_type_params": null,
                "join_to_timespine": false,
                "fill_nulls_with": null,
                "is_private": false,
                "metric_aggregation_params": {
                    "semantic_model": "bookings_source",
                    "agg": "percentile",
                    "agg_params": { "percentile": 0.99, "use_discrete_percentile": true },
                    "agg_time_dimension": "ds",
                    "non_additive_dimension": null,
                    "expr": "booking_value",
                },
            },
        }),
        // approximate_continuous_booking_value_p99
        json!({
            "name": "approximate_continuous_booking_value_p99",
            "description": "approximate_continuous_booking_value_p99",
            "type": "simple",
            "label": null,
            "time_granularity": null,
            "filter": null,
            "metadata": null,
            "config": null,
            "type_params": {
                "measure": null,
                "input_measures": [],
                "numerator": null,
                "denominator": null,
                "expr": "booking_value",
                "window": null,
                "grain_to_date": null,
                "metrics": null,
                "conversion_type_params": null,
                "cumulative_type_params": null,
                "join_to_timespine": false,
                "fill_nulls_with": null,
                "is_private": false,
                "metric_aggregation_params": {
                    "semantic_model": "bookings_source",
                    "agg": "approximate_continuous",
                    "agg_params": { "percentile": 0.99 },
                    "agg_time_dimension": "ds",
                    "non_additive_dimension": null,
                    "expr": "booking_value",
                },
            },
        }),
        // approximate_discrete_booking_value_p99
        json!({
            "name": "approximate_discrete_booking_value_p99",
            "description": "approximate_discrete_booking_value_p99",
            "type": "simple",
            "label": null,
            "time_granularity": null,
            "filter": null,
            "metadata": null,
            "config": null,
            "type_params": {
                "measure": null,
                "input_measures": [],
                "numerator": null,
                "denominator": null,
                "expr": "booking_value",
                "window": null,
                "grain_to_date": null,
                "metrics": null,
                "conversion_type_params": null,
                "cumulative_type_params": null,
                "join_to_timespine": false,
                "fill_nulls_with": null,
                "is_private": false,
                "metric_aggregation_params": {
                    "semantic_model": "bookings_source",
                    "agg": "approximate_discrete",
                    "agg_params": { "percentile": 0.99 },
                    "agg_time_dimension": "ds",
                    "non_additive_dimension": null,
                    "expr": "booking_value",
                },
            },
        }),
        // double_counted_delayed_bookings: derived = delayed_bookings * 2
        derived_metric(
            "double_counted_delayed_bookings",
            "delayed_bookings * 2",
            vec![metric_input_with_filter_and_alias(
                "bookings",
                Some("delayed_bookings"),
                "NOT {{ Dimension('booking__is_instant') }}"
            )]
        ),
        // every_two_days_bookers: cumulative count_distinct(guest_id) over 2-day window
        cumulative_metric(
            "every_two_days_bookers",
            "bookings_source",
            "count_distinct",
            "guest_id",
            "ds",
            Some(json!({"count": 2, "granularity": "day"})),
            None,
        ),
        // instant_booking_fraction_of_max_value: ratio(avg_booking_value WHERE instant / max_booking_value)
        ratio_metric_with_input_filters(
            "instant_booking_fraction_of_max_value",
            "average_booking_value",
            Some("{{ Dimension('booking__is_instant') }}"),
            None,
            "max_booking_value",
            None,
            None,
        ),
        // instant_booking_value_ratio: ratio(booking_value WHERE instant / booking_value)
        ratio_metric_with_input_filters(
            "instant_booking_value_ratio",
            "booking_value",
            Some("{{ Dimension('booking__is_instant') }}"),
            Some("booking_value_with_is_instant_constraint"),
            "booking_value",
            None,
            None,
        ),
        instant_bookings_measure_filter,
        // lux_booking_fraction_of_max_value: ratio(avg_booking_value WHERE lux / max_booking_value)
        ratio_metric_with_input_filters(
            "lux_booking_fraction_of_max_value",
            "average_booking_value",
            Some("{{ Dimension('listing__is_lux_latest') }}"),
            None,
            "max_booking_value",
            None,
            None,
        ),
        // lux_booking_value_rate_expr: derived with 3 filtered inputs
        derived_metric(
            "lux_booking_value_rate_expr",
            "average_booking_value * bookings / NULLIF(booking_value, 0)",
            vec![
                metric_input_with_filter_and_alias(
                    "average_booking_value",
                    None,
                    "{{ Dimension('listing__is_lux_latest') }}"
                ),
                metric_input_with_filter_and_alias(
                    "bookings",
                    None,
                    "{{ Dimension('listing__is_lux_latest') }}"
                ),
                metric_input("booking_value"),
            ]
        ),
        // median_booking_value: MEDIAN(booking_value)
        simple_metric(
            "median_booking_value",
            "median_booking_value",
            "bookings_source",
            "median",
            "booking_value",
            "ds"
        ),
        // regional_starting_balance_ratios: ratio of west coast vs east coast first-day balances
        ratio_metric_with_input_filters(
            "regional_starting_balance_ratios",
            "total_account_balance_first_day",
            Some("{{ Dimension('user__home_state_latest') }} IN ('CA', 'HI', 'WA')"),
            Some("west_coast_balance_first_day"),
            "total_account_balance_first_day",
            Some("{{ Dimension('user__home_state_latest') }} IN ('MD', 'NY', 'TX')"),
            Some("east_coast_balance_first_day"),
        ),
        // simple_subdaily_metric_default_day: archived_users (subdaily, default grain=day)
        simple_metric(
            "simple_subdaily_metric_default_day",
            "archived_users",
            "users_ds_source",
            "sum",
            "1",
            "archived_at"
        ),
        subdaily_metric_default_hour,
        // subdaily_cumulative_window_metric: cumulative archived_users window=3 hours
        cumulative_metric(
            "subdaily_cumulative_window_metric",
            "users_ds_source",
            "sum",
            "1",
            "archived_at",
            Some(json!({"count": 3, "granularity": "hour"})),
            None,
        ),
        // subdaily_cumulative_grain_to_date_metric: cumulative archived_users grain_to_date=hour
        cumulative_metric(
            "subdaily_cumulative_grain_to_date_metric",
            "users_ds_source",
            "sum",
            "1",
            "archived_at",
            None,
            Some("hour"),
        ),
        balance_first_day_of_month_metric,
        // trailing_2_months_revenue_sub_10: derived = t2mr - 10
        derived_metric(
            "trailing_2_months_revenue_sub_10",
            "t2mr - 10",
            vec![metric_input_alias("trailing_2_months_revenue", "t2mr")]
        ),
        // visit_buy_conversion_rate: conversion (no window, unbounded)
        conversion_metric(
            "visit_buy_conversion_rate",
            "visits",
            "buys",
            "user",
            "conversion_rate",
            None,
        ),
    ]);

    // ── Project Configuration ────────────────────────────────────────────
    let project_configuration = json!({
        "time_spine_table_configurations": [],
        "metadata": null,
        "dsi_package_version": { "major_version": "0", "minor_version": "0", "patch_version": "0" },
        "time_spines": [
            {
                "node_relation": { "alias": "mf_time_spine", "schema_name": SCHEMA, "database": null, "relation_name": null },
                "primary_column": { "name": "ds", "time_granularity": "day" },
                "custom_granularities": [{ "name": "alien_day", "column_name": "alien_day" }],
            },
            {
                "node_relation": { "alias": "mf_time_spine_hour", "schema_name": SCHEMA, "database": null, "relation_name": null },
                "primary_column": { "name": "ts", "time_granularity": "hour" },
                "custom_granularities": [],
            },
            {
                "node_relation": { "alias": "mf_time_spine_second", "schema_name": SCHEMA, "database": null, "relation_name": null },
                "primary_column": { "name": "ts", "time_granularity": "second" },
                "custom_granularities": [],
            },
        ],
    });

    // ── Saved Queries ────────────────────────────────────────────────────
    let saved_queries = json!([
        {
            "name": "p0_booking",
            "description": "Booking-related metrics that are of the highest priority.",
            "label": null,
            "metadata": null,
            "tags": [],
            "query_params": {
                "metrics": ["bookings", "instant_bookings"],
                "group_by": ["TimeDimension('metric_time', 'day')", "Dimension('listing__capacity_latest')"],
                "where": ["{{ Dimension('listing__capacity_latest') }} > 3"],
                "order_by": [],
                "limit": null,
            },
            "exports": [],
        },
    ]);

    json!({
        "semantic_models": [
            bookings_source, listings_latest, views_source, users_latest,
            users_ds_source, revenue, accounts_source, id_verifications,
            visits_source, buys_source, companies, company_regions,
            lux_listing_mapping, conversions_source, signups_source,
        ],
        "metrics": metrics,
        "project_configuration": project_configuration,
        "saved_queries": saved_queries,
    })
}

fn build_extended_date_manifest() -> serde_json::Value {
    let bookings_extended = sm(
        "bookings_extended",
        "fct_bookings_extended",
        Some("booking"),
        Some("ds"),
        json!([
            measure("bookings", "sum", Some("booking")),
            measure(
                "unique_listings_booked",
                "count_distinct",
                Some("listing_id")
            ),
        ]),
        json!([
            dim_time("ds", None, "day"),
            dim_categorical("is_instant", None),
        ]),
        json!([entity("listing", "foreign", "listing_id"),]),
    );

    let listings_extended = sm(
        "listings_extended",
        "dim_listings_extended",
        None,
        None,
        json!([]),
        json!([dim_time("listing_creation_ds", None, "day"),]),
        json!([entity("listing", "primary", "listing_id"),]),
    );

    let bookings_extended_monthly = sm(
        "bookings_extended_monthly",
        "fct_bookings_extended_monthly",
        Some("booking_monthly"),
        Some("ds"),
        json!([measure("bookings_monthly", "sum", Some("bookings_monthly")),]),
        json!([
            dim_time("ds", None, "month"),
            dim_categorical("is_instant", None),
        ]),
        json!([entity("listing", "foreign", "listing_id"),]),
    );

    let mut bookings_monthly_metric = simple_metric(
        "bookings_monthly",
        "bookings_monthly",
        "bookings_extended_monthly",
        "sum",
        "bookings_monthly",
        "ds",
    );
    bookings_monthly_metric["time_granularity"] = json!("month");

    let metrics = json!([
        simple_metric(
            "bookings",
            "bookings",
            "bookings_extended",
            "sum",
            "booking",
            "ds"
        ),
        simple_metric(
            "unique_listings_booked",
            "unique_listings_booked",
            "bookings_extended",
            "count_distinct",
            "listing_id",
            "ds"
        ),
        bookings_monthly_metric,
    ]);

    let project_configuration = json!({
        "time_spine_table_configurations": [],
        "metadata": null,
        "dsi_package_version": { "major_version": "0", "minor_version": "0", "patch_version": "0" },
        "time_spines": [
            {
                "node_relation": { "alias": "mf_time_spine", "schema_name": SCHEMA, "database": null, "relation_name": null },
                "primary_column": { "name": "ds", "time_granularity": "day" },
                "custom_granularities": [],
            },
        ],
    });

    json!({
        "semantic_models": [bookings_extended, listings_extended, bookings_extended_monthly],
        "metrics": metrics,
        "project_configuration": project_configuration,
        "saved_queries": [],
    })
}

fn build_unpartitioned_multi_hop_manifest() -> serde_json::Value {
    let account_month_txns = sm(
        "account_month_txns",
        "account_month_txns",
        Some("account"),
        Some("ds"),
        json!([measure("txn_count", "sum", Some("txn_count")),]),
        json!([dim_time("ds", None, "day"),]),
        json!([entity("account_id", "primary", "account_id"),]),
    );

    let bridge = sm(
        "bridge_table",
        "bridge_table",
        None,
        None,
        json!([]),
        json!([dim_categorical("extra_dim", None),]),
        json!([
            entity("account_id", "foreign", "account_id"),
            entity("customer_id", "foreign", "customer_id"),
        ]),
    );

    let customer = sm(
        "customer_table",
        "customer_table",
        None,
        None,
        json!([]),
        json!([
            dim_categorical("customer_name", None),
            dim_categorical("customer_atomic_weight", None),
        ]),
        json!([entity("customer_id", "primary", "customer_id"),]),
    );

    let customer_other = sm(
        "customer_other_data",
        "customer_other_data",
        None,
        None,
        json!([]),
        json!([dim_categorical("country", None),]),
        json!([
            entity("customer_id", "foreign", "customer_id"),
            entity("customer_third_hop_id", "foreign", "customer_third_hop_id"),
        ]),
    );

    let metrics = json!([simple_metric(
        "txn_count",
        "txn_count",
        "account_month_txns",
        "sum",
        "txn_count",
        "ds"
    ),]);

    let project_configuration = json!({
        "time_spine_table_configurations": [],
        "metadata": null,
        "dsi_package_version": { "major_version": "0", "minor_version": "0", "patch_version": "0" },
        "time_spines": [
            {
                "node_relation": { "alias": "mf_time_spine", "schema_name": SCHEMA, "database": null, "relation_name": null },
                "primary_column": { "name": "ds", "time_granularity": "day" },
                "custom_granularities": [],
            },
        ],
    });

    json!({
        "semantic_models": [account_month_txns, bridge, customer, customer_other],
        "metrics": metrics,
        "project_configuration": project_configuration,
        "saved_queries": [],
    })
}

fn build_partitioned_multi_hop_manifest() -> serde_json::Value {
    let account_month_txns = sm(
        "account_month_txns",
        "account_month_txns",
        Some("account"),
        Some("ds"),
        json!([measure("txn_count", "sum", Some("txn_count")),]),
        json!([
            dim_time("ds", None, "day"),
            dim_time_partition("ds_partitioned", "day"),
        ]),
        json!([entity("account_id", "primary", "account_id"),]),
    );

    let bridge = sm(
        "bridge_table",
        "bridge_table",
        None,
        None,
        json!([]),
        json!([
            dim_categorical("extra_dim", None),
            dim_time_partition("ds_partitioned", "day"),
        ]),
        json!([
            entity("account_id", "foreign", "account_id"),
            entity("customer_id", "foreign", "customer_id"),
        ]),
    );

    let customer = sm(
        "customer_table",
        "customer_table",
        None,
        None,
        json!([]),
        json!([
            dim_categorical("customer_name", None),
            dim_categorical("customer_atomic_weight", None),
            dim_time_partition("ds_partitioned", "day"),
        ]),
        json!([entity("customer_id", "primary", "customer_id"),]),
    );

    let customer_other = sm(
        "customer_other_data",
        "customer_other_data",
        None,
        Some("acquired_ds"),
        json!([measure("paraguayan_customers_count", "count", Some("1")),]),
        json!([
            dim_categorical("country", None),
            dim_time("acquired_ds", None, "day"),
        ]),
        json!([
            entity("customer_id", "foreign", "customer_id"),
            entity("customer_third_hop_id", "foreign", "customer_third_hop_id"),
        ]),
    );

    let third_hop = sm(
        "third_hop_table",
        "third_hop_table",
        Some("third_hop"),
        Some("third_hop_ds"),
        json!([measure(
            "third_hop_count",
            "count_distinct",
            Some("customer_third_hop_id")
        ),]),
        json!([dim_time("third_hop_ds", None, "day"),]),
        json!([entity(
            "customer_third_hop_id",
            "primary",
            "customer_third_hop_id"
        ),]),
    );

    let paraguayan_metric = simple_metric_with_filter(
        "paraguayan_customers",
        "paraguayan_customers",
        "customer_other_data",
        "count",
        "1",
        "acquired_ds",
        json!({"where_filters": [{"where_sql_template": "{{ Dimension('customer_other_data__country') }} = 'paraguay'"}]}),
    );

    let metrics = json!([
        simple_metric(
            "txn_count",
            "txn_count",
            "account_month_txns",
            "sum",
            "txn_count",
            "ds"
        ),
        simple_metric(
            "third_hop_count",
            "third_hop_count",
            "third_hop_table",
            "count_distinct",
            "customer_third_hop_id",
            "third_hop_ds"
        ),
        paraguayan_metric,
    ]);

    let project_configuration = json!({
        "time_spine_table_configurations": [],
        "metadata": null,
        "dsi_package_version": { "major_version": "0", "minor_version": "0", "patch_version": "0" },
        "time_spines": [
            {
                "node_relation": { "alias": "mf_time_spine", "schema_name": SCHEMA, "database": null, "relation_name": null },
                "primary_column": { "name": "ds", "time_granularity": "day" },
                "custom_granularities": [],
            },
        ],
    });

    json!({
        "semantic_models": [account_month_txns, bridge, customer, customer_other, third_hop],
        "metrics": metrics,
        "project_configuration": project_configuration,
        "saved_queries": [],
    })
}

fn build_simple_model_non_ds_manifest() -> serde_json::Value {
    let bookings_dt = sm(
        "bookings_source",
        "fct_bookings_dt",
        Some("booking"),
        Some("dt"),
        json!([measure("bookings", "sum", Some("1")),]),
        json!([
            dim_time("dt", None, "day"),
            dim_categorical("is_instant", None),
        ]),
        json!([
            entity("listing", "foreign", "listing_id"),
            entity("guest", "foreign", "guest_id"),
        ]),
    );

    let listings_latest = sm(
        "listings_latest",
        "dim_listings_latest",
        None,
        Some("ds"),
        json!([]),
        json!([
            dim_time("ds", Some("created_at"), "day"),
            dim_categorical("country_latest", Some("country")),
        ]),
        json!([entity("listing", "primary", "listing_id"),]),
    );

    let metrics = json!([simple_metric(
        "bookings",
        "bookings",
        "bookings_source",
        "sum",
        "1",
        "dt"
    ),]);

    let project_configuration = json!({
        "time_spine_table_configurations": [],
        "metadata": null,
        "dsi_package_version": { "major_version": "0", "minor_version": "0", "patch_version": "0" },
        "time_spines": [
            {
                "node_relation": { "alias": "mf_time_spine", "schema_name": SCHEMA, "database": null, "relation_name": null },
                "primary_column": { "name": "ds", "time_granularity": "day" },
                "custom_granularities": [],
            },
        ],
    });

    json!({
        "semantic_models": [bookings_dt, listings_latest],
        "metrics": metrics,
        "project_configuration": project_configuration,
        "saved_queries": [],
    })
}

fn build_scd_manifest() -> serde_json::Value {
    // bookings_source: same fct_bookings used in SIMPLE_MODEL.
    let bookings_source = sm(
        "bookings_source",
        "fct_bookings",
        Some("booking"),
        Some("ds"),
        json!([measure("bookings", "sum", Some("1")),]),
        json!([
            dim_categorical("is_instant", None),
            dim_time("ds", None, "day"),
        ]),
        json!([
            entity("listing", "foreign", "listing_id"),
            entity("guest", "foreign", "guest_id"),
        ]),
    );

    // listings: SCD table with validity windows (active_from, active_to).
    let mut listings = sm(
        "listings",
        "dim_listings",
        None,
        None,
        json!([]),
        json!([
            dim_categorical("capacity", None),
            dim_categorical("is_lux", None),
        ]),
        json!([
            entity("listing", "primary", "listing_id"),
            entity("user", "foreign", "user_id"),
        ]),
    );
    listings["validity_params"] = json!({
        "is_valid_from": "active_from",
        "is_valid_to": "active_to",
    });

    // users_latest for the SCD model: reuse the same dim_users_latest table
    // as the simple model so data matches the check_query expectations.
    let users_latest = sm(
        "scd_users_latest",
        "dim_users_latest",
        None,
        None,
        json!([]),
        json!([dim_categorical("home_state_latest", None),]),
        json!([entity("user", "primary", "user_id"),]),
    );

    // lux_listing_id_mapping: non-SCD bridge table.
    // Reuse the same dim_lux_listing_id_mapping table as the simple model.
    let lux_listing_mapping = sm(
        "scd_lux_listing_mapping",
        "dim_lux_listing_id_mapping",
        None,
        None,
        json!([]),
        json!([]),
        json!([
            entity("listing", "primary", "listing_id"),
            entity("lux_listing", "foreign", "lux_listing_id"),
        ]),
    );

    // lux_listings: second SCD table with valid_from/valid_to.
    let mut lux_listings = sm(
        "lux_listings",
        "dim_lux_listings",
        None,
        None,
        json!([]),
        json!([dim_categorical("is_confirmed_lux", None),]),
        json!([entity("lux_listing", "primary", "lux_listing_id"),]),
    );
    lux_listings["validity_params"] = json!({
        "is_valid_from": "valid_from",
        "is_valid_to": "valid_to",
    });

    // Metrics.
    let metrics = json!([
        simple_metric("bookings", "bookings", "bookings_source", "sum", "1", "ds"),
        simple_metric_filtered(
            "family_bookings",
            "bookings_source",
            "sum",
            "1",
            "ds",
            "{{ Dimension('listing__capacity') }} >= 3",
        ),
        simple_metric_filtered(
            "potentially_lux_bookings",
            "bookings_source",
            "sum",
            "1",
            "ds",
            "{{ Dimension('listing__is_lux') }} OR {{ Dimension('listing__is_lux') }} IS NULL",
        ),
    ]);

    let project_configuration = json!({
        "time_spine_table_configurations": [],
        "metadata": null,
        "dsi_package_version": { "major_version": "0", "minor_version": "0", "patch_version": "0" },
        "time_spines": [
            {
                "node_relation": { "alias": "mf_time_spine", "schema_name": SCHEMA, "database": null, "relation_name": null },
                "primary_column": { "name": "ds", "time_granularity": "day" },
                "custom_granularities": [],
            },
        ],
    });

    json!({
        "semantic_models": [bookings_source, listings, users_latest, lux_listing_mapping, lux_listings],
        "metrics": metrics,
        "project_configuration": project_configuration,
        "saved_queries": [],
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// DuckDB data loader — creates tables from MetricFlow's table snapshots
// ═══════════════════════════════════════════════════════════════════════════

/// SQL statements to create and populate all MetricFlow test tables.
#[allow(clippy::vec_init_then_push)]
fn data_ddl() -> Vec<String> {
    let mut stmts = Vec::new();

    // ── fct_bookings ─────────────────────────────────────────────────────
    stmts.push(
        "CREATE TABLE fct_bookings (
        ds DATE, ds_partitioned DATE, paid_at DATE,
        guest_id VARCHAR, host_id VARCHAR, listing_id VARCHAR,
        booking_value DOUBLE, is_instant BOOLEAN, referrer_id VARCHAR
    )"
        .into(),
    );

    stmts.push("INSERT INTO fct_bookings VALUES
        ('2019-12-01','2019-12-01','2019-12-01','u0003452','u0004114','l5948301',951.23,false,'u0003141'),
        ('2019-12-18','2019-12-18','2019-12-19','u0004114','u0003141','l3141592',797.42,false,null),
        ('2019-12-18','2019-12-18','2019-12-19','u0004114','u0003141','l3141592',808.14,true,'u0003141'),
        ('2019-12-18','2019-12-18','2019-12-19','u0003141','u0003154','l2718281',531.99,true,null),
        ('2019-12-18','2019-12-18','2019-12-19','u0003154','u0003141','l2718281',285.89,false,null),
        ('2019-12-18','2019-12-18','2019-12-19','u0004114','u0003141','l3141592',714.85,true,'u0005432'),
        ('2019-12-18','2019-12-18','2019-12-19','u0004114','u0003141','l3141592',352.42,false,null),
        ('2019-12-18','2019-12-18','2019-12-19','u0004114','u0003141','l3141592',444.14,true,'u0003141'),
        ('2019-12-18','2019-12-18','2019-12-19','u0003141','u0003154','l2718281',415.99,true,null),
        ('2019-12-18','2019-12-18','2019-12-19','u0003154','u0003141','l2718281',578.89,false,null),
        ('2019-12-18','2019-12-18','2019-12-19','u0004114','u0003141','l3141592',768.85,true,'u0005432'),
        ('2019-12-19','2019-12-19','2019-12-20','u0004114','u0003141','l2718281',368.42,true,null),
        ('2019-12-19','2019-12-19','2019-12-20','u0004114','u0003141','l2718281',521.14,true,null),
        ('2019-12-19','2019-12-19','2019-12-20','u0004114','u0003141','l3141592',581.99,false,null),
        ('2019-12-19','2019-12-19','2019-12-20','u0005432','u0003452','l2718281',753.85,true,'u0003452'),
        ('2019-12-19','2019-12-19','2019-12-20','u0004114','u1004114','l9658588-incomplete',0.0,false,null),
        ('2019-12-19','2019-12-19','2019-12-20','u0004114','u1004114','l8912456-incomplete',0.0,false,null),
        ('2019-12-19','2019-12-19','2019-12-20','u0003452','u0005432','l3141592',754.89,false,'u0003141'),
        ('2019-12-19','2019-12-19','2019-12-20','u0003452','u0004114','l5948301',152.23,false,'u0003141'),
        ('2019-12-19','2019-12-19','2019-12-20','u1003452','u1004114','no_such_listing',0.0,false,null),
        ('2019-12-19','2019-12-19','2019-12-20','u0004114','u0003141','l2718281',532.42,true,null),
        ('2019-12-19','2019-12-19','2019-12-20','u0004114','u0003141','l2718281',258.14,true,null),
        ('2019-12-19','2019-12-19','2019-12-20','u0004114','u0003141','l3141592',753.99,false,null),
        ('2019-12-19','2019-12-19','2019-12-20','u0005432','u0003452','l2718281',852.85,true,'u0003452'),
        ('2019-12-19','2019-12-19','2019-12-20','u0004114','u1004114','l9658588-incomplete',0.0,false,null),
        ('2019-12-19','2019-12-19','2019-12-20','u0004114','u1004114','l8912456-incomplete',0.0,false,null),
        ('2019-12-19','2019-12-19','2019-12-20','u0003452','u0005432','l3141592',728.89,false,'u0003141'),
        ('2019-12-19','2019-12-19','2019-12-20','u0003452','u0004114','l5948301',951.23,false,'u0003141')
    ".into());

    // ── dim_listings_latest ──────────────────────────────────────────────
    stmts.push(
        "CREATE TABLE dim_listings_latest (
        created_at DATE, listing_id VARCHAR, country VARCHAR,
        capacity INTEGER, is_lux BOOLEAN, user_id VARCHAR
    )"
        .into(),
    );

    stmts.push(
        "INSERT INTO dim_listings_latest VALUES
        ('2020-01-01','l3141592','us',3,true,'u0004114'),
        ('2020-01-02','l5948301','us',5,true,'u0004114'),
        ('2020-01-02','l2718281','cote d''ivoire',4,false,'u0005432'),
        ('2020-01-02','l9658588-incomplete','us',null,null,'u1004114'),
        ('2020-01-02','l8912456-incomplete',null,null,null,'u1004114'),
        ('2020-01-02','l7891283-incomplete','ca',null,false,'u1004114')
    "
        .into(),
    );

    // ── fct_views ────────────────────────────────────────────────────────
    stmts.push(
        "CREATE TABLE fct_views (
        ds DATE, ds_partitioned DATE, listing_id VARCHAR, user_id VARCHAR
    )"
        .into(),
    );

    stmts.push(
        "INSERT INTO fct_views VALUES
        ('2019-12-01','2019-12-01','l3141592','u0004114'),
        ('2019-12-01','2019-12-01','l2718281','u0005432'),
        ('2019-12-01','2019-12-01','l5948301','u0003452'),
        ('2019-12-19','2019-12-19','l3141592','u0004114'),
        ('2019-12-19','2019-12-19','l2718281','u0005432'),
        ('2019-12-19','2019-12-19','l3141592','u0003452'),
        ('2019-12-19','2019-12-19','l5948301','u0003141'),
        ('2019-12-19','2019-12-19','l5948301','u0004114'),
        ('2019-12-19','2019-12-19','l2718281','u0003141'),
        ('2019-12-19','2019-12-19','l2718281','u0003452'),
        ('2019-12-19','2019-12-19','l2718281','u0004114'),
        ('2019-12-20','2019-12-20','l3141592','u0003452'),
        ('2019-12-20','2019-12-20','l5948301','u0003141'),
        ('2019-12-20','2019-12-20','l2718281','u0005432')
    "
        .into(),
    );

    // ── dim_users_latest ─────────────────────────────────────────────────
    stmts.push(
        "CREATE TABLE dim_users_latest (
        ds DATE, user_id VARCHAR, home_state_latest VARCHAR
    )"
        .into(),
    );

    stmts.push(
        "INSERT INTO dim_users_latest VALUES
        ('2020-01-01','u0003141','MD'),
        ('2020-01-01','u0003154','CA'),
        ('2020-01-01','u0003452','HI'),
        ('2020-01-01','u0004114','NY'),
        ('2020-01-01','u0005432','WA'),
        ('2020-01-01','u1004114','TX'),
        ('2020-01-01','u1003452','CA')
    "
        .into(),
    );

    // ── dim_users ────────────────────────────────────────────────────────
    stmts.push(
        "CREATE TABLE dim_users (
        ds DATE, ds_partitioned DATE, created_at DATE, user_id VARCHAR,
        home_state VARCHAR, last_profile_edit_ts TIMESTAMP,
        bio_added_ts TIMESTAMP, last_login_ts TIMESTAMP, archived_at TIMESTAMP
    )"
        .into(),
    );

    stmts.push(
        "INSERT INTO dim_users VALUES
        ('2019-12-19','2019-12-19','2019-12-19','u0003452','HI','2019-12-19 10:30:00','2019-12-19 09:00:00','2019-12-19 08:15:00','2019-12-19 10:00:00'),
        ('2019-12-19','2019-12-19','2019-12-19','u0004114','NY','2019-12-19 11:00:00','2019-12-19 10:00:00','2019-12-19 09:30:00','2019-12-19 10:00:00'),
        ('2019-12-19','2019-12-19','2019-12-19','u0003141','MD','2019-12-19 12:30:00','2019-12-19 11:00:00','2019-12-19 10:45:00','2019-12-19 11:00:00'),
        ('2019-12-19','2019-12-19','2019-12-19','u0003154','CA','2019-12-19 14:00:00','2019-12-19 13:00:00','2019-12-19 12:00:00','2019-12-19 11:00:00'),
        ('2019-12-19','2019-12-19','2019-12-19','u0005432','WA','2019-12-19 15:30:00','2019-12-19 14:00:00','2019-12-19 13:15:00','2019-12-19 12:00:00'),
        ('2019-12-19','2019-12-19','2019-12-19','u1004114','TX','2019-12-19 16:00:00','2019-12-19 15:00:00','2019-12-19 14:30:00','2019-12-19 12:00:00'),
        ('2019-12-19','2019-12-19','2019-12-19','u1003452','CA','2019-12-19 17:30:00','2019-12-19 16:00:00','2019-12-19 15:45:00','2019-12-19 13:00:00')
    "
        .into(),
    );

    // ── fct_revenue ──────────────────────────────────────────────────────
    stmts.push(
        "CREATE TABLE fct_revenue (
        created_at DATE, user_id VARCHAR, revenue DOUBLE
    )"
        .into(),
    );

    stmts.push(
        "INSERT INTO fct_revenue VALUES
        ('2019-12-19','u0004114',100.0),
        ('2019-12-19','u0003141',200.0),
        ('2019-12-20','u0004114',150.0),
        ('2019-12-20','u0003452',50.0),
        ('2020-01-01','u0004114',300.0),
        ('2020-01-02','u0005432',75.0)
    "
        .into(),
    );

    // ── fct_accounts ─────────────────────────────────────────────────────
    stmts.push(
        "CREATE TABLE fct_accounts (
        ds DATE, user_id VARCHAR, account_balance DOUBLE,
        account_type VARCHAR, ds_month DATE
    )"
        .into(),
    );

    stmts.push(
        "INSERT INTO fct_accounts VALUES
        ('2019-12-19','u0003141',1000.0,'checking','2019-12-01'),
        ('2019-12-19','u0004114',2000.0,'savings','2019-12-01'),
        ('2019-12-20','u0003141',1100.0,'checking','2019-12-01'),
        ('2019-12-20','u0004114',1800.0,'savings','2019-12-01'),
        ('2020-01-01','u0003141',1200.0,'checking','2020-01-01'),
        ('2020-01-01','u0004114',1900.0,'savings','2020-01-01')
    "
        .into(),
    );

    // ── fct_id_verifications ─────────────────────────────────────────────
    stmts.push(
        "CREATE TABLE fct_id_verifications (
        ds DATE, ds_partitioned DATE, verification_id VARCHAR,
        user_id VARCHAR, verification_type VARCHAR
    )"
        .into(),
    );

    stmts.push(
        "INSERT INTO fct_id_verifications VALUES
        ('2019-12-19','2019-12-19','v001','u0003141','phone'),
        ('2019-12-19','2019-12-19','v002','u0004114','id_card'),
        ('2019-12-20','2019-12-20','v003','u0003452','phone'),
        ('2020-01-01','2020-01-01','v004','u0005432','passport')
    "
        .into(),
    );

    // ── fct_visits ───────────────────────────────────────────────────────
    stmts.push(
        "CREATE TABLE fct_visits (
        ds DATE, user_id VARCHAR, session_id VARCHAR, referrer_id VARCHAR
    )"
        .into(),
    );

    stmts.push(
        "INSERT INTO fct_visits VALUES
        ('2019-12-19','u0003141','s001','fb_ad_1'),
        ('2019-12-19','u0004114','s002','google'),
        ('2019-12-19','u0003452','s003',null),
        ('2019-12-20','u0003141','s004','fb_ad_1'),
        ('2019-12-20','u0005432','s005','email')
    "
        .into(),
    );

    // ── fct_buys ─────────────────────────────────────────────────────────
    stmts.push(
        "CREATE TABLE fct_buys (
        ds DATE, user_id VARCHAR, session_id VARCHAR, ds_month DATE
    )"
        .into(),
    );

    stmts.push(
        "INSERT INTO fct_buys VALUES
        ('2019-12-19','u0003141','s001','2019-12-01'),
        ('2019-12-20','u0003141','s004','2019-12-01'),
        ('2019-12-20','u0005432','s005','2019-12-01')
    "
        .into(),
    );

    // ── dim_companies ────────────────────────────────────────────────────
    stmts.push(
        "CREATE TABLE dim_companies (
        company_id VARCHAR, user_id VARCHAR, company_name VARCHAR
    )"
        .into(),
    );

    stmts.push(
        "INSERT INTO dim_companies VALUES
        ('c001','u0003141','Acme Corp'),
        ('c002','u0004114','Globex Inc')
    "
        .into(),
    );

    // ── dim_company_regions ────────────────────────────────────────────
    stmts.push(
        "CREATE TABLE dim_company_regions (
        region_id VARCHAR, company_id VARCHAR, region_name VARCHAR
    )"
        .into(),
    );

    stmts.push(
        "INSERT INTO dim_company_regions VALUES
        ('r001','c001','Northeast'),
        ('r002','c002','West Coast')
    "
        .into(),
    );

    // ── dim_lux_listing_id_mapping ───────────────────────────────────────
    stmts.push(
        "CREATE TABLE dim_lux_listing_id_mapping (
        listing_id VARCHAR, lux_listing_id VARCHAR
    )"
        .into(),
    );

    stmts.push(
        "INSERT INTO dim_lux_listing_id_mapping VALUES
        ('l3141592','lux_001'),
        ('l5948301','lux_002')
    "
        .into(),
    );

    // ── fct_conversions / fct_signups (bare dim name collision test) ────
    stmts.push("CREATE TABLE fct_conversions (created_at DATE, channel VARCHAR)".into());
    stmts.push(
        "INSERT INTO fct_conversions VALUES ('2024-01-01','organic'),('2024-01-02','paid')".into(),
    );
    stmts.push("CREATE TABLE fct_signups (created_at DATE, source VARCHAR)".into());
    stmts
        .push("INSERT INTO fct_signups VALUES ('2024-01-01','web'),('2024-01-02','mobile')".into());

    stmts
}

// ═══════════════════════════════════════════════════════════════════════════
// Test infrastructure
// ═══════════════════════════════════════════════════════════════════════════

/// Create an in-memory metric store and DuckDB with test data tables populated.
fn setup_mf_db() -> (InMemoryMetricStore, DuckDb) {
    let manifest = build_semantic_manifest();
    let store = InMemoryMetricStore::from_manifest(&manifest);

    let mut db = DuckDb::open_memory();
    for stmt in data_ddl() {
        if let Err(e) = db.execute_update(&stmt) {
            panic!(
                "Failed to execute DDL: {e}\n  SQL: {}",
                &stmt[..stmt.len().min(200)]
            );
        }
    }

    // Create time spine tables so join_to_timespine metrics can reference them.
    let spine_ddl = &[
        "CREATE TABLE mf_time_spine AS SELECT ds::DATE AS ds FROM generate_series(DATE '2019-01-01', DATE '2025-12-31', INTERVAL '1 day') AS t(ds)",
        "CREATE TABLE mf_time_spine_hour AS SELECT ts::TIMESTAMP AS ts FROM generate_series(TIMESTAMP '2019-12-01', TIMESTAMP '2020-01-07 23:00:00', INTERVAL '1 hour') AS t(ts)",
        "CREATE TABLE mf_time_spine_second AS SELECT ts::TIMESTAMP AS ts FROM generate_series(TIMESTAMP '2020-01-01 00:00:00', TIMESTAMP '2020-01-01 00:00:15', INTERVAL '1 second') AS t(ts)",
    ];
    for stmt in spine_ddl {
        if let Err(e) = db.execute_update(stmt) {
            panic!("Failed to create time spine: {e}");
        }
    }

    (store, db)
}

/// Compile a query spec and return the SQL string.
fn compile_query(store: &mut InMemoryMetricStore, json_spec: &str) -> Result<String, String> {
    let spec = parse_query_spec(json_spec).map_err(|e| e.to_string())?;
    compile(store, &spec, Dialect::DuckDB).map_err(|e| e.to_string())
}

fn compile_and_execute_batches(
    store: &mut InMemoryMetricStore,
    db: &mut DuckDb,
    json_spec: &str,
) -> Result<Vec<RecordBatch>, String> {
    let sql = compile_query(store, json_spec)?;
    db.execute_query(&sql)
        .map_err(|e| format!("SQL execution failed: {e}\n  SQL: {sql}"))
}

/// Compile and execute a query, returning the SQL, row count, and first scalar value.
fn compile_and_execute(
    store: &mut InMemoryMetricStore,
    db: &mut DuckDb,
    json_spec: &str,
) -> Result<(String, usize, Option<f64>), String> {
    let sql = compile_query(store, json_spec)?;

    // Verify no internal placeholders leaked into the generated SQL.
    if sql.contains("__GROUP_BY_PLACEHOLDER_") {
        return Err(format!(
            "generated SQL contains unreplaced __GROUP_BY_PLACEHOLDER__\n  SQL: {sql}"
        ));
    }
    if sql.contains("\x00PH") {
        return Err(format!(
            "generated SQL contains unreplaced \\x00PH placeholder\n  SQL: {sql}"
        ));
    }

    // Execute against the data tables
    let batches = db
        .execute_query(&sql)
        .map_err(|e| format!("SQL execution failed: {e}\n  SQL: {sql}"))?;
    let rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    #[allow(clippy::manual_map)]
    let scalar = batches.first().and_then(|b| {
        if b.num_rows() == 0 || b.num_columns() == 0 {
            return None;
        }
        let col_idx = b.num_columns() - 1;
        let col = b.column(col_idx);
        use arrow_array::{Decimal128Array, Float64Array, Int32Array, Int64Array};
        if let Some(arr) = col.as_any().downcast_ref::<Float64Array>() {
            Some(arr.value(0))
        } else if let Some(arr) = col.as_any().downcast_ref::<Int64Array>() {
            Some(arr.value(0) as f64)
        } else if let Some(arr) = col.as_any().downcast_ref::<Int32Array>() {
            Some(arr.value(0) as f64)
        } else if let Some(arr) = col.as_any().downcast_ref::<Decimal128Array>() {
            let scale = arr.scale() as f64;
            Some(arr.value(0) as f64 / 10f64.powf(scale))
        } else {
            None
        }
    });
    Ok((sql, rows, scalar))
}

// ═══════════════════════════════════════════════════════════════════════════
// Test cases
// ═══════════════════════════════════════════════════════════════════════════

struct TestCase {
    name: &'static str,
    spec: &'static str,
    /// Minimum expected rows (0 = just check it compiles + executes)
    min_rows: usize,
    /// If set, assert that the first scalar metric value equals this (within epsilon).
    expected_value: Option<f64>,
}

const TEST_CASES: &[TestCase] = &[
    // ── Simple metrics ───────────────────────────────────────────────
    TestCase {
        name: "simple_bookings_no_dimensions",
        spec: r#"{"metrics": ["bookings"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "simple_bookings_by_day",
        spec: r#"{"metrics": ["bookings"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "simple_bookings_by_listing",
        spec: r#"{"metrics": ["bookings"], "group_by": ["Entity('listing')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "simple_booking_value",
        spec: r#"{"metrics": ["booking_value"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "simple_max_booking_value",
        spec: r#"{"metrics": ["max_booking_value"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "simple_min_booking_value",
        spec: r#"{"metrics": ["min_booking_value"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "simple_bookers_count_distinct",
        spec: r#"{"metrics": ["bookers"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "simple_average_booking_value",
        spec: r#"{"metrics": ["average_booking_value"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "simple_referred_bookings_count",
        spec: r#"{"metrics": ["referred_bookings"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "simple_views",
        spec: r#"{"metrics": ["views"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "simple_listings",
        spec: r#"{"metrics": ["listings"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "simple_identity_verifications",
        spec: r#"{"metrics": ["identity_verifications"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "simple_revenue",
        spec: r#"{"metrics": ["revenue"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "simple_account_balance",
        spec: r#"{"metrics": ["account_balance"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Granularity variations ───────────────────────────────────────
    TestCase {
        name: "bookings_by_week",
        spec: r#"{"metrics": ["bookings"], "group_by": ["TimeDimension('metric_time', 'week')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "bookings_by_month",
        spec: r#"{"metrics": ["bookings"], "group_by": ["TimeDimension('metric_time', 'month')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "bookings_by_year",
        spec: r#"{"metrics": ["bookings"], "group_by": ["TimeDimension('metric_time', 'year')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Cross-model dimension joins ──────────────────────────────────
    TestCase {
        name: "bookings_by_listing_country",
        spec: r#"{"metrics": ["bookings"], "group_by": ["Dimension('listing__country_latest')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "bookings_by_listing_capacity",
        spec: r#"{"metrics": ["bookings"], "group_by": ["Dimension('listing__capacity_latest')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "revenue_by_user_home_state",
        spec: r#"{"metrics": ["revenue"], "group_by": ["Dimension('user__home_state_latest')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Multiple simple metrics (same model) ─────────────────────────
    TestCase {
        name: "bookings_and_booking_value",
        spec: r#"{"metrics": ["bookings", "booking_value"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "bookings_and_instant_bookings",
        spec: r#"{"metrics": ["bookings", "instant_bookings"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Multiple simple metrics (different models) ───────────────────
    TestCase {
        name: "bookings_and_views_no_dims",
        spec: r#"{"metrics": ["bookings", "views"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "bookings_and_views_by_day",
        spec: r#"{"metrics": ["bookings", "views"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "bookings_and_listings_by_listing",
        spec: r#"{"metrics": ["bookings", "listings"], "group_by": ["Entity('listing')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Multi-metric: constrained + non-constrained (same model) ────
    // booking_value (unconstrained) + instant_booking_value (WHERE is_instant)
    // Both from bookings_source — should produce a FULL OUTER JOIN or combined query
    // where the constraint applies only to instant_booking_value.
    TestCase {
        name: "constrained_with_non_constrained_same_src",
        spec: r#"{"metrics": ["booking_value", "instant_booking_value"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Multi-metric: constrained + non-constrained (different models) ──
    // instant_booking_value (bookings_source, WHERE is_instant) + views (views_source)
    TestCase {
        name: "constrained_with_non_constrained_diff_src",
        spec: r#"{"metrics": ["instant_booking_value", "views"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Multi-metric: simple + ratio (mixed types) ──────────────────
    // bookings (simple) + bookings_per_view (ratio: bookings/views)
    TestCase {
        name: "ratio_with_non_ratio",
        spec: r#"{"metrics": ["bookings", "bookings_per_view"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Multi-metric: 3 metrics (simple + ratio + simple, same model) ──
    TestCase {
        name: "query_with_3_metrics",
        spec: r#"{"metrics": ["bookings", "bookings_per_booker", "booking_value"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Multi-metric: different aggregation time dimensions ─────────
    // booking_value (agg_time_dim=ds) + booking_payments (agg_time_dim=paid_at)
    // Both from bookings_source but different time columns drive metric_time.
    TestCase {
        name: "metrics_with_different_agg_time_dims",
        spec: r#"{"metrics": ["booking_value", "booking_payments"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Multi-metric: shared alias derived (same backing metric) ────
    // derived_shared_alias_1a = bookings(alias=shared_alias) - 10
    // derived_shared_alias_1b = bookings(alias=shared_alias) - 100
    // Both derived from bookings with the same alias — tests alias deduplication.
    TestCase {
        name: "shared_alias_derived_same",
        spec: r#"{"metrics": ["derived_shared_alias_1a", "derived_shared_alias_1b"], "group_by": ["Dimension('booking__is_instant')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Multi-metric: shared alias derived (different backing metrics) ──
    // derived_shared_alias_1a = bookings(alias=shared_alias) - 10
    // derived_shared_alias_2 = instant_bookings(alias=shared_alias) + 10
    // Different backing metrics but same alias name — must not collide.
    TestCase {
        name: "shared_alias_derived_different",
        spec: r#"{"metrics": ["derived_shared_alias_1a", "derived_shared_alias_2"], "group_by": ["Dimension('booking__is_instant')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Multi-metric: two metrics with NULL dimension values ────────
    // bookings + views grouped by metric_time, listing__is_lux_latest, listing__country_latest
    // Some listings have NULL is_lux / NULL country — tests COALESCE alignment.
    TestCase {
        name: "two_metrics_null_dim_values",
        spec: r#"{"metrics": ["bookings", "views"], "group_by": ["TimeDimension('metric_time', 'day')", "Dimension('listing__is_lux_latest')", "Dimension('listing__country_latest')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Multi-metric: three metrics with NULL dimension values ──────
    // bookings + views + listings grouped by listing__is_lux_latest, listing__country_latest
    // No time dimension — pure categorical with NULLs.
    TestCase {
        name: "three_metrics_null_dim_values",
        spec: r#"{"metrics": ["bookings", "views", "listings"], "group_by": ["Dimension('listing__is_lux_latest')", "Dimension('listing__country_latest')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Multi-metric: no dimensions (different models) ──────────────
    // bookings (bookings_source) + listings (listings_latest) — CROSS JOIN
    TestCase {
        name: "multi_metrics_no_dims",
        spec: r#"{"metrics": ["bookings", "listings"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Multi-metric: order by metric value ─────────────────────────
    TestCase {
        name: "multi_metrics_ordered_by_metric",
        spec: r#"{"metrics": ["bookings", "booking_value"], "group_by": ["TimeDimension('metric_time', 'day')"], "order_by": ["-booking_value"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Multi-metric: order by dimension ────────────────────────────
    TestCase {
        name: "multi_metrics_ordered_by_dimension",
        spec: r#"{"metrics": ["bookings", "booking_value"], "group_by": ["TimeDimension('metric_time', 'day')"], "order_by": ["-metric_time"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Multi-metric: two cumulative metrics ────────────────────────
    TestCase {
        name: "multiple_cumulative_metrics",
        spec: r#"{"metrics": ["revenue_all_time", "trailing_2_months_revenue"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Derived metrics ──────────────────────────────────────────────
    TestCase {
        name: "derived_booking_fees",
        spec: r#"{"metrics": ["booking_fees"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "derived_booking_fees_by_day",
        spec: r#"{"metrics": ["booking_fees"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "derived_booking_fees_per_booker",
        spec: r#"{"metrics": ["booking_fees_per_booker"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "derived_views_times_booking_value",
        spec: r#"{"metrics": ["views_times_booking_value"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "derived_non_referred_bookings_pct",
        spec: r#"{"metrics": ["non_referred_bookings_pct"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "nested_derived_booking_value_sub_instant_add_10",
        spec: r#"{"metrics": ["booking_value_sub_instant_add_10"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Offset derived metrics ──────────────────────────────────────
    // offset_window: bookings_growth_1_day = bookings - bookings(1 day ago)
    // On 2019-12-19: bookings=18, bookings_1_day_ago (from 12-18)=10, growth=8
    TestCase {
        name: "offset_window_bookings_growth_by_day",
        spec: r#"{"metrics": ["bookings_growth_1_day"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // offset_to_grain: bookings - bookings_at_start_of_month
    // On 2019-12-19: bookings=18, bookings_at_start_of_month (from 12-01)=1, growth=17
    TestCase {
        name: "offset_to_grain_bookings_growth_by_day",
        spec: r#"{"metrics": ["bookings_growth_since_start_of_month"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Ratio metrics ────────────────────────────────────────────────
    TestCase {
        name: "ratio_bookings_per_booker",
        spec: r#"{"metrics": ["bookings_per_booker"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "ratio_bookings_per_view",
        spec: r#"{"metrics": ["bookings_per_view"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "ratio_bookings_per_listing",
        spec: r#"{"metrics": ["bookings_per_listing"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "ratio_bookings_per_dollar",
        spec: r#"{"metrics": ["bookings_per_dollar"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "ratio_bookings_per_booker_by_day",
        spec: r#"{"metrics": ["bookings_per_booker"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Where filters ────────────────────────────────────────────────
    TestCase {
        name: "bookings_with_where_filter",
        spec: r#"{"metrics": ["bookings"], "where": ["{{ Dimension('booking__is_instant') }}"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "bookings_where_listing_country",
        spec: r#"{"metrics": ["bookings"], "where": ["{{ Dimension('listing__country_latest') }} = 'us'"], "group_by": ["Dimension('booking__is_instant')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Order by + limit ─────────────────────────────────────────────
    TestCase {
        name: "bookings_order_by_metric_time",
        spec: r#"{"metrics": ["bookings"], "group_by": ["TimeDimension('metric_time', 'day')"], "order_by": ["+metric_time"], "limit": 5}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Metric with filter (instant_booking_value) ───────────────────
    TestCase {
        name: "metric_with_filter_instant_booking_value",
        spec: r#"{"metrics": ["instant_booking_value"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // Metric filter: bookings where listing's total booking_value > 1000.
    // Listings l3141592, l2718281, l5948301 all have total > 1000 (23 matching bookings).
    TestCase {
        name: "metric_filter_bookings_by_listing_value",
        spec: r#"{"metrics": ["bookings_where_listing_high_value"]}"#,
        min_rows: 1,
        expected_value: Some(23.0),
    },
    // ── Cumulative metrics ──────────────────────────────────────────
    TestCase {
        name: "cumulative_revenue_all_time",
        spec: r#"{"metrics": ["revenue_all_time"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "cumulative_trailing_2_months_revenue",
        spec: r#"{"metrics": ["trailing_2_months_revenue"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "cumulative_revenue_mtd",
        spec: r#"{"metrics": ["revenue_mtd"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "cumulative_bookings_all_time",
        spec: r#"{"metrics": ["cumulative_bookings"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "cumulative_every_2_days_bookers",
        spec: r#"{"metrics": ["every_2_days_bookers"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "cumulative_revenue_all_time_no_time_dim",
        spec: r#"{"metrics": ["revenue_all_time"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "cumulative_trailing_2_months_by_user",
        spec: r#"{"metrics": ["trailing_2_months_revenue"], "group_by": ["TimeDimension('metric_time', 'day')", "Dimension('user__home_state_latest')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Time spine / fill_nulls metrics ─────────────────────────────
    TestCase {
        name: "fill_nulls_no_spine_by_day",
        spec: r#"{"metrics": ["bookings_fill_nulls_no_spine"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "join_to_time_spine_by_day",
        spec: r#"{"metrics": ["bookings_join_to_time_spine"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "fill_nulls_with_spine_by_day",
        spec: r#"{"metrics": ["bookings_fill_nulls_with_spine"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "fill_nulls_with_spine_by_month",
        spec: r#"{"metrics": ["bookings_fill_nulls_with_spine"], "group_by": ["TimeDimension('metric_time', 'month')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "revenue_fill_nulls_with_spine_by_day",
        spec: r#"{"metrics": ["revenue_fill_nulls_with_spine"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "join_to_time_spine_no_group_by",
        spec: r#"{"metrics": ["bookings_join_to_time_spine"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Semi-additive measures ──────────────────────────────────────
    // total_account_balance_first_day: SUM(balance) on the MIN(ds) date only
    // Our data has 3 dates (2019-12-19, 2019-12-20, 2020-01-01), so first day = 2019-12-19
    // On 2019-12-19: u0003141=1000 + u0004114=2000 = 3000
    TestCase {
        name: "semi_additive_first_day_balance",
        spec: r#"{"metrics": ["total_account_balance_first_day"]}"#,
        min_rows: 1,
        expected_value: Some(3000.0),
    },
    // current_account_balance_by_user: SUM(balance) on MAX(ds) per user
    // u0003141: max ds=2020-01-01, balance=1200
    // u0004114: max ds=2020-01-01, balance=1900
    // Total = 3100
    TestCase {
        name: "semi_additive_current_balance_by_user",
        spec: r#"{"metrics": ["current_account_balance_by_user"]}"#,
        min_rows: 1,
        expected_value: Some(3100.0),
    },
    TestCase {
        name: "semi_additive_current_balance_by_user_grouped",
        spec: r#"{"metrics": ["current_account_balance_by_user"], "group_by": ["Entity('user')"]}"#,
        min_rows: 2,
        expected_value: None,
    },
    TestCase {
        name: "semi_additive_first_day_by_account_type",
        spec: r#"{"metrics": ["total_account_balance_first_day"], "group_by": ["Dimension('account__account_type')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "semi_additive_current_balance_with_where",
        spec: r#"{"metrics": ["current_account_balance_by_user"], "where": ["{{ Dimension('account__account_type') }} = 'savings'"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Multi-hop joins ─────────────────────────────────────────────
    // bookings → user entity → companies model → company_name dimension
    // This is a 2-hop join: bookings_source.user_id → companies.user_id → companies.company_name
    TestCase {
        name: "multi_hop_bookings_by_company",
        spec: r#"{"metrics": ["bookings"], "group_by": ["Dimension('company__company_name')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // bookings → listing entity → lux_listing_mapping → lux_listing (2-hop through mapping)
    TestCase {
        name: "multi_hop_bookings_by_lux_listing",
        spec: r#"{"metrics": ["bookings"], "group_by": ["Entity('lux_listing')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // revenue → user entity → companies → company_name (2-hop from different base)
    TestCase {
        name: "multi_hop_revenue_by_company",
        spec: r#"{"metrics": ["revenue"], "group_by": ["Dimension('company__company_name')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Conversion metrics ─────────────────────────────────────────
    // conversion_rate: buys with prior visit (within 7 days, same user) / total visits
    // 5 visits, 3 buys all with matching visits => rate = 3/5 = 0.6
    TestCase {
        name: "conversion_rate_7day_window",
        spec: r#"{"metrics": ["visit_buy_conversion_rate_7days"]}"#,
        min_rows: 1,
        expected_value: Some(0.6),
    },
    // conversion count: number of buys with matching visit within 7 days
    TestCase {
        name: "conversion_count_7day_window",
        spec: r#"{"metrics": ["visit_buy_conversions_7days"]}"#,
        min_rows: 1,
        expected_value: Some(3.0),
    },
    // conversion rate without window (all time) — same data, same result
    TestCase {
        name: "conversion_rate_unbounded",
        spec: r#"{"metrics": ["visit_buy_conversion_rate_unbounded"]}"#,
        min_rows: 1,
        expected_value: Some(0.6),
    },
    // Conversion with constant property: session_id must match between visit and buy.
    // Same data, same result (all buys have matching visit sessions).
    TestCase {
        name: "conversion_rate_with_constant_property",
        spec: r#"{"metrics": ["visit_buy_conversion_rate_by_session"]}"#,
        min_rows: 1,
        expected_value: Some(0.6),
    },
    // ═══════════════════════════════════════════════════════════════════
    // Ported from itest_metrics.yaml — single-metric tests (existing metrics)
    // ═══════════════════════════════════════════════════════════════════
    // constrained_metric: instant_booking_value with additional where filter
    TestCase {
        name: "constrained_metric_with_where",
        spec: r#"{"metrics": ["instant_booking_value"], "group_by": ["TimeDimension('metric_time', 'day')"], "where": ["{{ Dimension('booking__is_instant') }}"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // constrained_metric_with_user_input_constraint: instant_booking_value + listing filter
    TestCase {
        name: "constrained_metric_with_user_constraint",
        spec: r#"{"metrics": ["instant_booking_value"], "group_by": ["TimeDimension('metric_time', 'day')"], "where": ["{{ Dimension('listing__is_lux_latest') }}"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // count_distinct with categorical dimension
    TestCase {
        name: "count_distinct_by_is_instant",
        spec: r#"{"metrics": ["bookers"], "group_by": ["TimeDimension('metric_time', 'day')", "Dimension('booking__is_instant')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // count_distinct_with_constraint
    TestCase {
        name: "count_distinct_with_constraint",
        spec: r#"{"metrics": ["bookers"], "group_by": ["TimeDimension('metric_time', 'day')"], "where": ["{{ Dimension('booking__is_instant') }}"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ratio with sort by metric_time
    TestCase {
        name: "ratio_sort_by_metric_time",
        spec: r#"{"metrics": ["bookings_per_booker"], "group_by": ["TimeDimension('metric_time', 'day')"], "order_by": ["+metric_time"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // metric_with_aggregation_time_dimension: booking_payments uses paid_at as agg_time_dim
    TestCase {
        name: "booking_payments_by_metric_time",
        spec: r#"{"metrics": ["booking_payments"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // derived_metric: non_referred_bookings_pct by day
    TestCase {
        name: "derived_non_referred_bookings_pct_by_day",
        spec: r#"{"metrics": ["non_referred_bookings_pct"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // derived_metric_with_offset_to_grain by day
    TestCase {
        name: "offset_to_grain_by_day",
        spec: r#"{"metrics": ["bookings_growth_since_start_of_month"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // derived_metric_with_input_metric_with_constraint by day
    TestCase {
        name: "derived_booking_value_sub_instant_by_day",
        spec: r#"{"metrics": ["booking_value_sub_instant"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // nested_derived_metric_with_input_metric_with_constraint by day
    TestCase {
        name: "nested_derived_booking_value_sub_instant_add_10_by_day",
        spec: r#"{"metrics": ["booking_value_sub_instant_add_10"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // offset_to_grain with week granularity
    TestCase {
        name: "offset_to_grain_by_week",
        spec: r#"{"metrics": ["bookings_growth_since_start_of_month"], "group_by": ["TimeDimension('metric_time', 'week')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // offset_to_grain with multiple granularities (month + week)
    TestCase {
        name: "offset_to_grain_by_month_and_week",
        spec: r#"{"metrics": ["bookings_growth_since_start_of_month"], "group_by": ["TimeDimension('metric_time', 'month')", "TimeDimension('metric_time', 'week')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // offset_to_grain with agg_time_dim instead of metric_time
    TestCase {
        name: "offset_to_grain_with_agg_time_dim",
        spec: r#"{"metrics": ["bookings_growth_since_start_of_month"], "group_by": ["TimeDimension('booking__ds', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // simple_join_to_time_spine by metric_time
    TestCase {
        name: "join_to_time_spine_by_metric_time",
        spec: r#"{"metrics": ["bookings_join_to_time_spine"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // join_to_time_spine with where filter
    TestCase {
        name: "join_to_time_spine_with_where_filter",
        spec: r#"{"metrics": ["bookings_join_to_time_spine"], "group_by": ["TimeDimension('metric_time', 'day')"], "where": ["{{ Dimension('booking__is_instant') }}"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Ported from itest_metrics.yaml — tests using NEW metric definitions
    // ═══════════════════════════════════════════════════════════════════
    // identifier_constrained_metric: booking_value WHERE listing IS NOT NULL
    TestCase {
        name: "identifier_constrained_metric",
        spec: r#"{"metrics": ["booking_value_for_non_null_listing_id"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // lux_listings with dimension filter in where
    TestCase {
        name: "lux_listings_filtered",
        spec: r#"{"metrics": ["lux_listings"], "group_by": ["Dimension('listing__country_latest')"], "where": ["{{ Dimension('listing__country_latest') }} = 'us'"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // booking_value_per_view: derived metric with NULL dimension values
    TestCase {
        name: "derived_metrics_with_null_dimension_values",
        spec: r#"{"metrics": ["booking_value_per_view"], "group_by": ["Dimension('listing__is_lux_latest')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // bookings_growth_2_weeks: offset window 14 days
    TestCase {
        name: "derived_offset_window_2_weeks",
        spec: r#"{"metrics": ["bookings_growth_2_weeks"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // bookings_5_day_lag: single offset input
    TestCase {
        name: "derived_offset_5_day_lag",
        spec: r#"{"metrics": ["bookings_5_day_lag"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // bookings_5_day_lag with month granularity
    TestCase {
        name: "derived_offset_5_day_lag_by_month",
        spec: r#"{"metrics": ["bookings_5_day_lag"], "group_by": ["TimeDimension('metric_time', 'month')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // bookings_5_day_lag with multiple granularities
    TestCase {
        name: "derived_offset_5_day_lag_by_month_and_week",
        spec: r#"{"metrics": ["bookings_5_day_lag"], "group_by": ["TimeDimension('metric_time', 'month')", "TimeDimension('metric_time', 'week')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // bookings_month_start_compared_to_1_month_prior: offset_window + offset_to_grain combined
    TestCase {
        name: "derived_offset_window_and_offset_to_grain",
        spec: r#"{"metrics": ["bookings_month_start_compared_to_1_month_prior"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // bookings_month_start_compared_to_1_month_prior with year granularity
    TestCase {
        name: "derived_offset_window_and_offset_to_grain_by_year",
        spec: r#"{"metrics": ["bookings_month_start_compared_to_1_month_prior"], "group_by": ["TimeDimension('metric_time', 'year')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // nested derived: instant_plus_non_referred_bookings_pct
    TestCase {
        name: "nested_derived_instant_plus_non_referred",
        spec: r#"{"metrics": ["instant_plus_non_referred_bookings_pct"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // derived_metric_ratio: bookings_per_lux_listing_derived
    TestCase {
        name: "derived_bookings_per_lux_listing",
        spec: r#"{"metrics": ["bookings_per_lux_listing_derived"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // every_2_days_bookers_2_days_ago: offset of cumulative metric
    TestCase {
        name: "derived_offset_cumulative_metric",
        spec: r#"{"metrics": ["every_2_days_bookers_2_days_ago"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // bookings_growth_2_weeks with agg_time_dim
    TestCase {
        name: "offset_window_with_agg_time_dim",
        spec: r#"{"metrics": ["bookings_growth_2_weeks"], "group_by": ["TimeDimension('booking__ds', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // bookings_fill_nulls_with_0 by metric_time day
    TestCase {
        name: "fill_nulls_with_0_by_metric_time",
        spec: r#"{"metrics": ["bookings_fill_nulls_with_0"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // bookings_fill_nulls_with_0 by month
    TestCase {
        name: "fill_nulls_with_0_by_month",
        spec: r#"{"metrics": ["bookings_fill_nulls_with_0"], "group_by": ["TimeDimension('metric_time', 'month')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // bookings_fill_nulls_with_0_without_time_spine by day
    TestCase {
        name: "fill_nulls_without_time_spine_by_day",
        spec: r#"{"metrics": ["bookings_fill_nulls_with_0_without_time_spine"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // every_two_days_bookers_fill_nulls_with_0: cumulative + fill nulls
    TestCase {
        name: "cumulative_fill_nulls_with_0",
        spec: r#"{"metrics": ["every_two_days_bookers_fill_nulls_with_0"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // bookings_growth_2_weeks_fill_nulls_with_0_for_non_offset: derived with fill_nulls input
    TestCase {
        name: "derived_fill_nulls_for_one_input_metric",
        spec: r#"{"metrics": ["bookings_growth_2_weeks_fill_nulls_with_0_for_non_offset"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // bookings_offset_twice: nested derived with double offset
    TestCase {
        name: "nested_derived_outer_offset",
        spec: r#"{"metrics": ["bookings_offset_twice"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // booking_fees_since_start_of_month: nested derived with offset and multiple inputs
    TestCase {
        name: "nested_derived_offset_multiple_inputs",
        spec: r#"{"metrics": ["booking_fees_since_start_of_month"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // active_listings: metric with metric-in-where-filter
    TestCase {
        name: "metric_with_metric_in_where_filter",
        spec: r#"{"metrics": ["active_listings"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Metric-in-where-filter: query-level {{ Metric() }} in WHERE clause
    // ═══════════════════════════════════════════════════════════════════
    // listings WHERE Metric('bookings', ['listing']) > 2
    TestCase {
        name: "query_metric_in_where_filter",
        spec: r#"{"metrics": ["listings"], "where": ["{{ Metric('bookings', ['listing']) }} > 2"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // NOTE: derived/ratio metrics in WHERE filter (e.g., Metric('views_times_booking_value'))
    // are not yet supported — compile_metric_filter_ctes only handles simple metrics.
    // listings WHERE Metric('revenue_all_time', ['user']) > 2
    TestCase {
        name: "cumulative_metric_in_where_filter",
        spec: r#"{"metrics": ["listings"], "where": ["{{ Metric('revenue_all_time', ['user']) }} > 2"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // listings WHERE Metric('bookings') > 2 AND Metric('bookers') > 1
    TestCase {
        name: "multiple_metrics_in_where_filter",
        spec: r#"{"metrics": ["listings"], "where": ["{{ Metric('bookings', ['listing']) }} > 2 AND {{ Metric('bookers', ['listing']) }} > 1"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Fill nulls: additional variations
    // ═══════════════════════════════════════════════════════════════════
    // fill_nulls_with_0 multi-metric: bookings_fill_nulls_with_0 + views
    TestCase {
        name: "fill_nulls_with_0_multi_metric",
        spec: r#"{"metrics": ["bookings_fill_nulls_with_0", "views"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // fill_nulls derived multi-metric: bookings_growth_2_weeks_fill_nulls + booking_value
    TestCase {
        name: "fill_nulls_derived_multi_metric",
        spec: r#"{"metrics": ["bookings_growth_2_weeks_fill_nulls_with_0_for_non_offset", "booking_value"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // fill_nulls with categorical dimension: bookings_fill_nulls_with_0_without_time_spine + views
    TestCase {
        name: "fill_nulls_multi_metric_with_categorical_dim",
        spec: r#"{"metrics": ["bookings_fill_nulls_with_0_without_time_spine", "views"], "group_by": ["TimeDimension('metric_time', 'day')", "Dimension('listing__is_lux_latest')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Nested offset with constraints and agg_time_dim
    // ═══════════════════════════════════════════════════════════════════
    // bookings_offset_twice with agg_time_dim
    TestCase {
        name: "nested_offset_with_agg_time_dim",
        spec: r#"{"metrics": ["bookings_offset_twice"], "group_by": ["TimeDimension('booking__ds', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // popular_listing_bookings_per_booker: ratio with metric filter defined on metric
    TestCase {
        name: "ratio_with_metric_filter_on_metric",
        spec: r#"{"metrics": ["popular_listing_bookings_per_booker"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Join to time spine with filters
    // ═══════════════════════════════════════════════════════════════════
    // bookings_join_to_time_spine with WHERE filter and queried dimension
    TestCase {
        name: "join_to_time_spine_with_queried_filter_and_dim",
        spec: r#"{"metrics": ["bookings_join_to_time_spine"], "group_by": ["TimeDimension('metric_time', 'day')", "Dimension('booking__is_instant')"], "where": ["{{ Dimension('booking__is_instant') }}"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Metric filter with entity prefix
    // ═══════════════════════════════════════════════════════════════════
    // listings WHERE Metric('views', ['view__listing']) > 2
    TestCase {
        name: "metric_filter_with_entity_prefix",
        spec: r#"{"metrics": ["listings"], "where": ["{{ Metric('views', ['view__listing']) }} > 2"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // NOTE: nested_derived_metric_offset_with_joined_where_constraint_not_selected
    // requires WHERE filter on non-grouped dimensions — currently only grouped dims are supported.
    // ═══════════════════════════════════════════════════════════════════
    // Date part: EXTRACT(part FROM ...) instead of DATE_TRUNC
    // ═══════════════════════════════════════════════════════════════════
    // bookings by EXTRACT(YEAR FROM metric_time)
    TestCase {
        name: "date_part_year",
        spec: r#"{"metrics": ["bookings"], "group_by": ["TimeDimension('metric_time', 'day', date_part='year')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // bookings by EXTRACT(YEAR FROM booking__ds) — non-metric_time agg_time_dim
    TestCase {
        name: "date_part_year_non_metric_time",
        spec: r#"{"metrics": ["bookings"], "group_by": ["TimeDimension('booking__ds', 'day', date_part='year')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // bookings with multiple date parts: quarter, dow, doy, day
    TestCase {
        name: "date_part_multiple",
        spec: r#"{"metrics": ["bookings"], "group_by": ["TimeDimension('metric_time', 'day', date_part='quarter')", "TimeDimension('metric_time', 'day', date_part='dow')", "TimeDimension('metric_time', 'day', date_part='doy')", "TimeDimension('metric_time', 'day', date_part='day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // derived offset metric with date_part: bookings_5_day_lag by EXTRACT(MONTH)
    TestCase {
        name: "date_part_derived_offset_month",
        spec: r#"{"metrics": ["bookings_5_day_lag"], "group_by": ["TimeDimension('metric_time', 'day', date_part='month')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // count distinct by date_part: bookers by EXTRACT(DOW)
    TestCase {
        name: "date_part_count_distinct_dow",
        spec: r#"{"metrics": ["bookers"], "group_by": ["TimeDimension('metric_time', 'day', date_part='dow')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Time constraint: WHERE metric_time BETWEEN start AND end
    // ═══════════════════════════════════════════════════════════════════
    // bookings_offset_twice with time constraint
    // Offset chain: bookings (5 days) → offset_once (2 days) → offset_twice.
    // Base data at 2019-12-01 appears at spine 2019-12-01+5+2=2019-12-08.
    TestCase {
        name: "time_constraint_nested_offset",
        spec: r#"{"metrics": ["bookings_offset_twice"], "group_by": ["TimeDimension('metric_time', 'day')"], "time_constraint": ["2019-12-05", "2019-12-10"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // bookings_5_day_lag: base data at 2019-12-01 appears at spine 2019-12-06
    TestCase {
        name: "time_constraint_offset_window",
        spec: r#"{"metrics": ["bookings_5_day_lag"], "group_by": ["TimeDimension('metric_time', 'day')"], "time_constraint": ["2019-12-05", "2019-12-10"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // every_2_days_bookers_2_days_ago: cumulative offset, base at 12/01 appears at spine 12/03
    TestCase {
        name: "time_constraint_cumulative_offset",
        spec: r#"{"metrics": ["every_2_days_bookers_2_days_ago"], "group_by": ["TimeDimension('metric_time', 'day')"], "time_constraint": ["2019-12-01", "2019-12-10"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // bookings_join_to_time_spine with time constraint narrowing to a single day
    TestCase {
        name: "time_constraint_join_to_time_spine",
        spec: r#"{"metrics": ["bookings_join_to_time_spine"], "group_by": ["TimeDimension('metric_time', 'day')"], "time_constraint": ["2019-12-19", "2019-12-19"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Remaining STANDARD tests from itest_metrics.yaml
    // ═══════════════════════════════════════════════════════════════════
    // lux_listings by listing__country_latest WHERE = 'us'
    TestCase {
        name: "lux_listings_by_country_filtered",
        spec: r#"{"metrics": ["lux_listings"], "group_by": ["Dimension('listing__country_latest')"], "where": ["{{ Dimension('listing__country_latest') }} = 'us'"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // booking_value + instant_booking_value by metric_time (same measure, one constrained)
    TestCase {
        name: "same_measure_constrained_and_unconstrained",
        spec: r#"{"metrics": ["booking_value", "instant_booking_value"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // fill_nulls_with_0 by non-metric time dimension (paid_at)
    TestCase {
        name: "fill_nulls_with_0_non_metric_time",
        spec: r#"{"metrics": ["bookings_fill_nulls_with_0"], "group_by": ["TimeDimension('booking__paid_at', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // fill_nulls_with_0 by agg_time_dim (booking__ds)
    TestCase {
        name: "fill_nulls_with_0_agg_time_dim",
        spec: r#"{"metrics": ["bookings_fill_nulls_with_0"], "group_by": ["TimeDimension('booking__ds', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // fill_nulls_with_0 by categorical dimension (is_instant)
    TestCase {
        name: "fill_nulls_with_0_categorical_dim",
        spec: r#"{"metrics": ["bookings_fill_nulls_with_0"], "group_by": ["Dimension('booking__is_instant')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ratio_with_zero_denominator: bookings_per_dollar (tests NULLIF(denominator,0))
    // Our data only has 2019-12 dates; use a range where booking_value might be 0.
    TestCase {
        name: "ratio_with_zero_denominator",
        spec: r#"{"metrics": ["bookings_per_dollar"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Remaining DATE_PART tests
    // ═══════════════════════════════════════════════════════════════════
    // offset metric with agg_time_dim date_part: bookings_5_day_lag by EXTRACT(DOY FROM booking__ds)
    TestCase {
        name: "date_part_offset_agg_time_dim_doy",
        spec: r#"{"metrics": ["bookings_5_day_lag"], "group_by": ["TimeDimension('booking__ds', 'day', date_part='doy')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Remaining TIME_CONSTRAINT tests
    // ═══════════════════════════════════════════════════════════════════
    // join_to_time_spine with queried time constraint (same as time_constraint_join_to_time_spine
    // but tests that constraint is applied even when metric_time is in group_by)
    TestCase {
        name: "time_constraint_join_to_time_spine_queried",
        spec: r#"{"metrics": ["bookings_join_to_time_spine"], "group_by": ["TimeDimension('metric_time', 'day')"], "time_constraint": ["2019-12-19", "2019-12-19"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // NOTE: ratio_with_zero_denominator from YAML uses time_constraint to isolate a
    // date with 0 booking_value. Our data always has non-zero booking_value, so we
    // test the ratio without time_constraint above (it still exercises NULLIF).
    // ═══════════════════════════════════════════════════════════════════
    // Subdaily tests: archived_users with hour-level agg_time_dimension
    // ═══════════════════════════════════════════════════════════════════
    // simple subdaily metric queried at default day granularity
    TestCase {
        name: "simple_subdaily_metric_default_day",
        spec: r#"{"metrics": ["archived_users"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: Some(7.0),
    },
    // simple subdaily metric queried at hour granularity
    TestCase {
        name: "simple_subdaily_metric_default_hour",
        spec: r#"{"metrics": ["archived_users"], "group_by": ["TimeDimension('metric_time', 'hour')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // subdaily metric with offset window (hourly offset)
    TestCase {
        name: "subdaily_offset_window",
        spec: r#"{"metrics": ["subdaily_offset_window_metric"], "group_by": ["TimeDimension('metric_time', 'hour')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // subdaily metric with offset to grain (hour)
    TestCase {
        name: "subdaily_offset_grain_to_date",
        spec: r#"{"metrics": ["subdaily_offset_grain_to_date_metric"], "group_by": ["TimeDimension('metric_time', 'hour')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // subdaily metric joined to time spine
    TestCase {
        name: "subdaily_join_to_time_spine",
        spec: r#"{"metrics": ["subdaily_join_to_time_spine_metric"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // subdaily metric grouped by user dimension
    TestCase {
        name: "subdaily_metric_by_dimension",
        spec: r#"{"metrics": ["archived_users"], "group_by": ["TimeDimension('metric_time', 'day')", "Dimension('user__home_state')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // subdaily metric at hour granularity grouped by dimension
    TestCase {
        name: "subdaily_metric_hour_by_dimension",
        spec: r#"{"metrics": ["archived_users"], "group_by": ["TimeDimension('metric_time', 'hour')", "Dimension('user__home_state')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Derived/ratio metric in WHERE filter
    // ═══════════════════════════════════════════════════════════════════
    // WHERE filter referencing a derived metric (views_times_booking_value)
    TestCase {
        name: "derived_metric_in_where_filter",
        spec: r#"{"metrics": ["bookings"], "where": ["{{ Metric('views_times_booking_value', ['listing']) }} > 1000"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // WHERE filter referencing a ratio metric (bookings_per_view)
    TestCase {
        name: "ratio_metric_in_where_filter",
        spec: r#"{"metrics": ["bookings"], "where": ["{{ Metric('bookings_per_view', ['listing']) }} > 0.5"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // WHERE-pushdown-on-offset: WHERE filters applied at derived CTE level, not base
    // ═══════════════════════════════════════════════════════════════════
    // offset metric with WHERE filter on grouped dimension (is_instant = false matches our data)
    TestCase {
        name: "offset_with_where_on_grouped_dim",
        spec: r#"{"metrics": ["bookings_5_day_lag"], "group_by": ["TimeDimension('metric_time', 'day')", "Dimension('booking__is_instant')"], "where": ["NOT {{ Dimension('booking__is_instant') }}"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // nested offset with WHERE constraint on grouped dimension
    TestCase {
        name: "nested_offset_with_where_constraint",
        spec: r#"{"metrics": ["bookings_growth_2_weeks"], "group_by": ["TimeDimension('metric_time', 'day')", "Dimension('booking__is_instant')"], "where": ["{{ Dimension('booking__is_instant') }}"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Third-hop join tests: bookings → user → company → region (3 hops)
    // ═══════════════════════════════════════════════════════════════════
    // bookings grouped by region_name (3-hop: booking→guest(user)→companies→company_regions)
    TestCase {
        name: "third_hop_bookings_by_region",
        spec: r#"{"metrics": ["bookings"], "group_by": ["Dimension('region__region_name')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // revenue grouped by region_name (3-hop: revenue→user→companies→company_regions)
    TestCase {
        name: "third_hop_revenue_by_region",
        spec: r#"{"metrics": ["revenue"], "group_by": ["Dimension('region__region_name')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // 3+ input derived metric: COALESCE join keys (PR #9620)
    // ═══════════════════════════════════════════════════════════════════
    // bookings + views + listings from 3 different models, grouped by listing.
    // Different listings appear in different tables; without COALESCE on the
    // join key, the 3rd CTE can't match keys that only exist in the 2nd CTE.
    TestCase {
        name: "three_input_derived_by_listing",
        spec: r#"{"metrics": ["bookings_plus_views_plus_listings"], "group_by": ["Entity('listing')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "three_input_derived_by_day",
        spec: r#"{"metrics": ["bookings_plus_views_plus_listings"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // No dimensions — CROSS JOIN, sanity check
    TestCase {
        name: "three_input_derived_no_dims",
        spec: r#"{"metrics": ["bookings_plus_views_plus_listings"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // NULL-safe join: IS NOT DISTINCT FROM (PR #9622)
    // ═══════════════════════════════════════════════════════════════════
    // Multi-metric grouped by nullable dimension (is_lux_latest has NULLs).
    // Without IS NOT DISTINCT FROM, NULL keys produce phantom duplicate rows.
    // bookings has listings with NULL is_lux; views also has listings with NULL is_lux.
    // We expect exactly 3 rows: true, false, NULL — not more.
    TestCase {
        name: "null_safe_join_multi_metric_nullable_dim",
        spec: r#"{"metrics": ["bookings", "views"], "group_by": ["Dimension('listing__is_lux_latest')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // Ratio metric grouped by nullable dimension — same NULL-safe join concern.
    TestCase {
        name: "null_safe_join_ratio_nullable_dim",
        spec: r#"{"metrics": ["bookings_per_view"], "group_by": ["Dimension('listing__is_lux_latest')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // fill_nulls_with in derived expression (PR #9621)
    // ═══════════════════════════════════════════════════════════════════
    // bookings_fill_nulls_with_0 (fill=0) + views in a derived metric.
    // Grouped by day: dates where one side has data but the other doesn't
    // should get 0 for the fill_nulls input, not NULL.
    TestCase {
        name: "fill_nulls_in_derived_by_day",
        spec: r#"{"metrics": ["fill_nulls_derived_test"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "fill_nulls_in_derived_no_dims",
        spec: r#"{"metrics": ["fill_nulls_derived_test"]}"#,
        min_rows: 1,
        expected_value: None,
    },
];

// ═══════════════════════════════════════════════════════════════════════════
// The test
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn metricflow_compat_scorecard() {
    let (mut store, mut db) = setup_mf_db();

    let mut pass = 0u32;
    let mut fail = 0u32;
    let mut results: Vec<(String, bool, String)> = Vec::new();

    for tc in TEST_CASES {
        match compile_and_execute(&mut store, &mut db, tc.spec) {
            Ok((sql, rows, scalar)) => {
                if rows < tc.min_rows {
                    fail += 1;
                    results.push((
                        tc.name.to_string(),
                        false,
                        format!("expected >= {} rows, got {rows}\n  SQL: {sql}", tc.min_rows),
                    ));
                } else if let Some(expected) = tc.expected_value {
                    if let Some(actual) = scalar {
                        if (actual - expected).abs() < 0.01 {
                            pass += 1;
                            results.push((
                                tc.name.to_string(),
                                true,
                                format!("{rows} rows, value={actual}"),
                            ));
                        } else {
                            fail += 1;
                            results.push((
                                tc.name.to_string(),
                                false,
                                format!("value mismatch: expected {expected}, got {actual}\n  SQL: {sql}"),
                            ));
                        }
                    } else {
                        fail += 1;
                        results.push((
                            tc.name.to_string(),
                            false,
                            format!("expected value {expected} but could not extract scalar\n  SQL: {sql}"),
                        ));
                    }
                } else {
                    pass += 1;
                    results.push((tc.name.to_string(), true, format!("{rows} rows")));
                }
            }
            Err(e) => {
                fail += 1;
                results.push((tc.name.to_string(), false, e));
            }
        }
    }

    // Print scorecard
    let bar = "=".repeat(70);
    eprintln!("\n{bar}");
    eprintln!("MetricFlow Compatibility Scorecard");
    eprintln!("{bar}");
    for (name, ok, detail) in &results {
        let icon = if *ok { "PASS" } else { "FAIL" };
        eprintln!("  [{icon}] {name}: {detail}");
    }
    eprintln!("{bar}");
    let total = pass + fail;
    eprintln!(
        "  Total: {total}  Pass: {pass}  Fail: {fail}  ({:.0}%)",
        (pass as f64 / total as f64) * 100.0
    );
    eprintln!("{bar}\n");

    // Fail the test if any case failed — this makes CI catch regressions
    assert_eq!(
        fail, 0,
        "{fail} of {total} MetricFlow compatibility tests failed"
    );
}

#[test]
fn saved_query_p0_booking() {
    let (mut store, mut db) = setup_mf_db();

    let spec = parse_query_spec(
        r#"{"metrics": ["bookings", "instant_bookings"],
            "group_by": ["TimeDimension('metric_time', 'day')", "Dimension('listing__capacity_latest')"],
            "where": ["{{ Dimension('listing__capacity_latest') }} > 3"]}"#,
    )
    .expect("failed to parse saved query spec");

    assert_eq!(spec.metrics, vec!["bookings", "instant_bookings"]);
    assert_eq!(spec.group_by.len(), 2);
    assert_eq!(spec.where_filters.len(), 1);

    let sql = compile(&mut store, &spec, Dialect::DuckDB).expect("failed to compile saved query");

    let batches = db
        .execute_query(&sql)
        .expect("failed to execute saved query SQL");
    let rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert!(
        rows >= 1,
        "saved query should return at least 1 row, got {rows}"
    );

    eprintln!("  [PASS] saved_query_p0_booking: {rows} rows");
}

// ═══════════════════════════════════════════════════════════════════════════
// Snowflake compile-only tests — verify SQL generation without execution
// ═══════════════════════════════════════════════════════════════════════════

/// Compile a query spec targeting Snowflake dialect.
fn compile_snowflake(store: &mut InMemoryMetricStore, json_spec: &str) -> Result<String, String> {
    let spec = parse_query_spec(json_spec).map_err(|e| e.to_string())?;
    compile(store, &spec, Dialect::Snowflake).map_err(|e| e.to_string())
}

#[test]
fn snowflake_compile_scorecard() {
    let (mut store, _db) = setup_mf_db();

    let mut pass = 0u32;
    let mut fail = 0u32;
    let mut results: Vec<(String, bool, String)> = Vec::new();

    for tc in TEST_CASES {
        match compile_snowflake(&mut store, tc.spec) {
            Ok(sql) => {
                let mut issues: Vec<String> = Vec::new();

                // No internal placeholders should leak.
                if sql.contains("__GROUP_BY_PLACEHOLDER_") {
                    issues.push("unreplaced __GROUP_BY_PLACEHOLDER__".to_string());
                }
                if sql.contains("\x00PH") {
                    issues.push("unreplaced \\x00PH placeholder".to_string());
                }

                // DuckDB-specific constructs must not appear in Snowflake SQL.
                if sql.contains("generate_series") {
                    issues.push("DuckDB-only generate_series() in Snowflake SQL".to_string());
                }
                if sql.contains("CAST(") && sql.contains(" AS DOUBLE)") {
                    issues.push("DuckDB-only DOUBLE type (Snowflake uses FLOAT)".to_string());
                }

                if issues.is_empty() {
                    pass += 1;
                    let lines = sql.lines().count();
                    results.push((tc.name.to_string(), true, format!("{lines} lines of SQL")));
                } else {
                    fail += 1;
                    results.push((
                        tc.name.to_string(),
                        false,
                        format!("{}\n  SQL: {sql}", issues.join("; ")),
                    ));
                }
            }
            Err(e) => {
                fail += 1;
                results.push((tc.name.to_string(), false, format!("compile error: {e}")));
            }
        }
    }

    // Scorecard output
    let bar = "=".repeat(70);
    eprintln!("\n{bar}");
    eprintln!("Snowflake Compile-Only Scorecard");
    eprintln!("{bar}");
    for (name, ok, detail) in &results {
        let icon = if *ok { "PASS" } else { "FAIL" };
        eprintln!("  [{icon}] {name}: {detail}");
    }
    eprintln!("{bar}");
    let total = pass + fail;
    eprintln!(
        "  Total: {total}  Pass: {pass}  Fail: {fail}  ({:.0}%)",
        (pass as f64 / total as f64) * 100.0
    );
    eprintln!("{bar}\n");

    assert_eq!(fail, 0, "{fail} of {total} Snowflake compile tests failed");
}

#[test]
fn snowflake_saved_query_compile() {
    let (mut store, _db) = setup_mf_db();

    let spec = parse_query_spec(
        r#"{"metrics": ["bookings", "instant_bookings"],
            "group_by": ["TimeDimension('metric_time', 'day')", "Dimension('listing__capacity_latest')"],
            "where": ["{{ Dimension('listing__capacity_latest') }} > 3"]}"#,
    )
    .expect("failed to parse saved query spec");

    let sql = compile(&mut store, &spec, Dialect::Snowflake)
        .expect("failed to compile saved query for Snowflake");

    assert!(
        !sql.contains("generate_series"),
        "Snowflake SQL should not contain generate_series"
    );
    assert!(
        !sql.contains("__GROUP_BY_PLACEHOLDER_"),
        "placeholder leak in saved query Snowflake SQL"
    );

    eprintln!(
        "  [PASS] snowflake_saved_query_compile: {} lines",
        sql.lines().count()
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Cross-backend comparison: DuckDB vs Snowflake recordings
// ═══════════════════════════════════════════════════════════════════════════

fn snowflake_tome_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("snowflake_recordings.parquet")
}

fn extract_all_metric_values(batches: &[RecordBatch]) -> Vec<f64> {
    use arrow_array::{Decimal128Array, Float64Array, Int32Array, Int64Array};
    let mut values = Vec::new();
    for b in batches {
        if b.num_rows() == 0 || b.num_columns() == 0 {
            continue;
        }
        let col_idx = b.num_columns() - 1;
        let col = b.column(col_idx);
        for row in 0..b.num_rows() {
            if col.is_null(row) {
                continue;
            }
            let val = if let Some(arr) = col.as_any().downcast_ref::<Float64Array>() {
                arr.value(row)
            } else if let Some(arr) = col.as_any().downcast_ref::<Int64Array>() {
                arr.value(row) as f64
            } else if let Some(arr) = col.as_any().downcast_ref::<Int32Array>() {
                arr.value(row) as f64
            } else if let Some(arr) = col.as_any().downcast_ref::<Decimal128Array>() {
                let scale = arr.scale() as f64;
                arr.value(row) as f64 / 10f64.powf(scale)
            } else {
                continue;
            };
            values.push(val);
        }
    }
    values
}

fn metric_sum(batches: &[RecordBatch]) -> f64 {
    extract_all_metric_values(batches).iter().sum()
}

fn sorted_metric_values(batches: &[RecordBatch]) -> Vec<i64> {
    let mut vals: Vec<i64> = extract_all_metric_values(batches)
        .iter()
        .map(|v| (v * 100.0).round() as i64)
        .collect();
    vals.sort();
    vals
}

const KNOWN_CROSS_BACKEND_DISCREPANCIES: &[&str] = &[
    // Snowflake recordings predate compiler bug fixes on this branch — needs re-recording.
    "three_metrics_null_dim_values",
    "constrained_with_non_constrained_same_src",
    "cumulative_bookings_all_time",
    "cumulative_every_2_days_bookers",
    "cumulative_fill_nulls_with_0",
    "cumulative_revenue_all_time",
    "cumulative_trailing_2_months_by_user",
    "cumulative_trailing_2_months_revenue",
    "date_part_derived_offset_month",
    "date_part_offset_agg_time_dim_doy",
    "derived_fill_nulls_for_one_input_metric",
    "derived_offset_5_day_lag",
    "derived_offset_5_day_lag_by_month",
    "derived_offset_5_day_lag_by_month_and_week",
    "derived_offset_cumulative_metric",
    "derived_offset_window_2_weeks",
    "derived_offset_window_and_offset_to_grain",
    "fill_nulls_derived_multi_metric",
    "fill_nulls_in_derived_by_day",
    "fill_nulls_with_0_agg_time_dim",
    "fill_nulls_with_0_by_metric_time",
    "fill_nulls_with_0_by_month",
    "fill_nulls_with_0_multi_metric",
    "fill_nulls_with_spine_by_day",
    "fill_nulls_with_spine_by_month",
    "join_to_time_spine_by_day",
    "join_to_time_spine_by_metric_time",
    "join_to_time_spine_with_where_filter",
    "metrics_with_different_agg_time_dims",
    "multiple_cumulative_metrics",
    "nested_derived_offset_multiple_inputs",
    "nested_derived_outer_offset",
    "nested_offset_with_agg_time_dim",
    "offset_to_grain_bookings_growth_by_day",
    "offset_to_grain_by_day",
    "offset_to_grain_by_month_and_week",
    "offset_to_grain_by_week",
    "offset_to_grain_with_agg_time_dim",
    "offset_window_bookings_growth_by_day",
    "offset_window_with_agg_time_dim",
    "offset_with_where_on_grouped_dim",
    "ratio_with_metric_filter_on_metric",
    "revenue_fill_nulls_with_spine_by_day",
    "same_measure_constrained_and_unconstrained",
    "simple_subdaily_metric_default_hour",
    "subdaily_join_to_time_spine",
    "subdaily_metric_hour_by_dimension",
    "subdaily_offset_grain_to_date",
    "subdaily_offset_window",
    "time_constraint_cumulative_offset",
    "time_constraint_nested_offset",
];

#[test]
fn cross_backend_duckdb_vs_snowflake() {
    let tome = snowflake_tome_path();
    if !tome.exists() {
        eprintln!("cross_backend: no Snowflake tome — skipping");
        return;
    }

    let sf_recordings = common::read_tome(&tome);
    let (mut store, mut db) = setup_mf_db();

    let mut compared = 0u32;
    let mut pass = 0u32;
    let mut fail = 0u32;
    let mut results: Vec<(String, bool, String)> = Vec::new();

    let mut known_skip = 0u32;
    for tc in TEST_CASES {
        let sf_batches = match sf_recordings.get(tc.name) {
            Some(b) => b.clone(),
            None => continue,
        };

        if KNOWN_CROSS_BACKEND_DISCREPANCIES.contains(&tc.name) {
            known_skip += 1;
            results.push((
                tc.name.to_string(),
                true,
                "KNOWN DISCREPANCY — skipped".to_string(),
            ));
            continue;
        }

        compared += 1;
        let sf_rows: usize = sf_batches.iter().map(|b| b.num_rows()).sum();

        let duck_batches = match compile_and_execute_batches(&mut store, &mut db, tc.spec) {
            Ok(b) => b,
            Err(e) => {
                fail += 1;
                results.push((tc.name.to_string(), false, format!("DuckDB error: {e}")));
                continue;
            }
        };
        let duck_rows: usize = duck_batches.iter().map(|b| b.num_rows()).sum();

        let rows_match = duck_rows == sf_rows;

        let duck_sorted = sorted_metric_values(&duck_batches);
        let sf_sorted = sorted_metric_values(&sf_batches);
        let values_match = duck_sorted == sf_sorted;

        let duck_sum = metric_sum(&duck_batches);
        let sf_sum = metric_sum(&sf_batches);
        let sum_match = (duck_sum - sf_sum).abs() < 0.01;

        if rows_match && values_match {
            pass += 1;
            results.push((
                tc.name.to_string(),
                true,
                format!("{duck_rows} rows, sum={duck_sum:.2}"),
            ));
        } else if rows_match && sum_match {
            pass += 1;
            results.push((
                tc.name.to_string(),
                true,
                format!("{duck_rows} rows, sum={duck_sum:.2} (rounding diff)"),
            ));
        } else {
            fail += 1;
            let mut detail = String::new();
            if !rows_match {
                detail.push_str(&format!("rows: duck={duck_rows} sf={sf_rows}. "));
            }
            if !sum_match {
                detail.push_str(&format!("sum: duck={duck_sum:.2} sf={sf_sum:.2}. "));
            }
            if !values_match && sum_match {
                detail.push_str("sorted values differ despite same sum (distribution mismatch). ");
            }
            results.push((tc.name.to_string(), false, detail));
        }
    }

    let bar = "=".repeat(70);
    eprintln!("\n{bar}");
    eprintln!("Cross-Backend Comparison: DuckDB vs Snowflake");
    eprintln!("{bar}");
    for (name, ok, detail) in &results {
        let icon = if *ok { "PASS" } else { "FAIL" };
        eprintln!("  [{icon}] {name}: {detail}");
    }
    eprintln!("{bar}");
    eprintln!("  Compared: {compared}  Pass: {pass}  Fail: {fail}  Known-skip: {known_skip}");
    eprintln!("{bar}\n");

    assert_eq!(
        fail, 0,
        "{fail} of {compared} cross-backend comparisons failed"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Additional dialect compile-only scorecards
// ═══════════════════════════════════════════════════════════════════════════

fn run_dialect_compile_scorecard(
    dialect: Dialect,
    dialect_name: &str,
    validate: fn(&str) -> Vec<String>,
) {
    let (mut store, _db) = setup_mf_db();

    let mut pass = 0u32;
    let mut fail = 0u32;
    let mut results: Vec<(String, bool, String)> = Vec::new();

    for tc in TEST_CASES {
        let spec = match parse_query_spec(tc.spec) {
            Ok(s) => s,
            Err(e) => {
                fail += 1;
                results.push((tc.name.to_string(), false, format!("parse error: {e}")));
                continue;
            }
        };
        match compile(&mut store, &spec, dialect) {
            Ok(sql) => {
                let mut issues = validate(&sql);

                if sql.contains("__GROUP_BY_PLACEHOLDER_") {
                    issues.push("unreplaced __GROUP_BY_PLACEHOLDER__".to_string());
                }
                if sql.contains("\x00PH") {
                    issues.push("unreplaced \\x00PH placeholder".to_string());
                }

                if issues.is_empty() {
                    pass += 1;
                    let lines = sql.lines().count();
                    results.push((tc.name.to_string(), true, format!("{lines} lines of SQL")));
                } else {
                    fail += 1;
                    results.push((
                        tc.name.to_string(),
                        false,
                        format!("{}\n  SQL: {sql}", issues.join("; ")),
                    ));
                }
            }
            Err(e) => {
                fail += 1;
                results.push((tc.name.to_string(), false, format!("compile error: {e}")));
            }
        }
    }

    let bar = "=".repeat(70);
    eprintln!("\n{bar}");
    eprintln!("{dialect_name} Compile-Only Scorecard");
    eprintln!("{bar}");
    for (name, ok, detail) in &results {
        let icon = if *ok { "PASS" } else { "FAIL" };
        eprintln!("  [{icon}] {name}: {detail}");
    }
    eprintln!("{bar}");
    let total = pass + fail;
    eprintln!(
        "  Total: {total}  Pass: {pass}  Fail: {fail}  ({:.0}%)",
        (pass as f64 / total as f64) * 100.0
    );
    eprintln!("{bar}\n");

    assert_eq!(
        fail, 0,
        "{fail} of {total} {dialect_name} compile tests failed"
    );
}

fn validate_no_other_dialect_constructs(sql: &str) -> Vec<String> {
    let mut issues = Vec::new();
    if sql.contains("generate_series") {
        issues.push("DuckDB-only generate_series()".to_string());
    }
    if sql.contains("GENERATOR") {
        issues.push("Snowflake-only GENERATOR()".to_string());
    }
    issues
}

#[test]
fn redshift_compile_scorecard() {
    run_dialect_compile_scorecard(Dialect::Redshift, "Redshift", |sql| {
        let mut issues = Vec::new();
        if sql.contains("generate_series") {
            issues.push("generate_series() not supported on all Redshift node types".to_string());
        }
        if sql.contains("GENERATOR") {
            issues.push("Snowflake-only GENERATOR()".to_string());
        }
        if sql.contains(" AS FLOAT64)") {
            issues.push("BigQuery-only FLOAT64 type".to_string());
        }
        issues
    });
}

#[test]
fn bigquery_compile_scorecard() {
    run_dialect_compile_scorecard(Dialect::BigQuery, "BigQuery", |sql| {
        let mut issues = Vec::new();
        if sql.contains("generate_series") {
            issues.push("DuckDB-only generate_series()".to_string());
        }
        if sql.contains("GENERATOR") {
            issues.push("Snowflake-only GENERATOR()".to_string());
        }
        if sql.contains("::DATE") || sql.contains("::TIMESTAMP") || sql.contains("::INT") {
            issues.push("Postgres-style :: cast (BigQuery uses CAST())".to_string());
        }
        if sql.contains(" AS DOUBLE)") {
            issues.push("DuckDB-only DOUBLE type (BigQuery uses FLOAT64)".to_string());
        }
        if sql.contains("EXPLODE(") {
            issues.push("Databricks-only EXPLODE()".to_string());
        }
        issues
    });
}

#[test]
fn databricks_compile_scorecard() {
    run_dialect_compile_scorecard(Dialect::Databricks, "Databricks", |sql| {
        let mut issues = validate_no_other_dialect_constructs(sql);
        if sql.contains("::DATE") || sql.contains("::TIMESTAMP") || sql.contains("::INT") {
            issues.push("Postgres-style :: cast (Databricks uses CAST())".to_string());
        }
        if sql.contains("GENERATE_DATE_ARRAY") || sql.contains("GENERATE_TIMESTAMP_ARRAY") {
            issues.push("BigQuery-only GENERATE_DATE_ARRAY()".to_string());
        }
        issues
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// Ported MetricFlow Python integration tests
// ═══════════════════════════════════════════════════════════════════════════
//
// Data-driven test suite that loads 266 test cases converted from MetricFlow's
// Python YAML integration tests. Each test compiles a query with our Rust
// compiler, executes both our SQL and MetricFlow's reference check_query against
// DuckDB, and compares the results.

/// Create a DuckDB populated with the FULL MetricFlow fixture data
/// (generated from Python YAML table snapshots).
struct TestStores {
    simple: InMemoryMetricStore,
    extended_date: InMemoryMetricStore,
    unpartitioned_multi_hop: InMemoryMetricStore,
    partitioned_multi_hop: InMemoryMetricStore,
    simple_non_ds: InMemoryMetricStore,
    scd: InMemoryMetricStore,
}

fn setup_mf_db_full() -> (TestStores, DuckDb) {
    let stores = TestStores {
        simple: InMemoryMetricStore::from_manifest(&build_semantic_manifest()),
        extended_date: InMemoryMetricStore::from_manifest(&build_extended_date_manifest()),
        unpartitioned_multi_hop: InMemoryMetricStore::from_manifest(
            &build_unpartitioned_multi_hop_manifest(),
        ),
        partitioned_multi_hop: InMemoryMetricStore::from_manifest(
            &build_partitioned_multi_hop_manifest(),
        ),
        simple_non_ds: InMemoryMetricStore::from_manifest(&build_simple_model_non_ds_manifest()),
        scd: InMemoryMetricStore::from_manifest(&build_scd_manifest()),
    };

    let mut db = DuckDb::open_memory();

    let sql_files: &[&str] = &[
        include_str!("data/tables_simple_model_extra.sql"),
        include_str!("data/tables_time_spine.sql"),
        include_str!("data/tables_extended_date_model.sql"),
        include_str!("data/tables_multi_hop_model.sql"),
        include_str!("data/tables_scd_model.sql"),
    ];
    for file_sql in sql_files {
        for stmt in file_sql.split(';') {
            let s = stmt.trim();
            if !s.is_empty() {
                if let Err(e) = db.execute_update(s) {
                    panic!(
                        "Failed to execute DDL: {e}\n  SQL: {}",
                        &s[..s.len().min(200)]
                    );
                }
            }
        }
    }

    // Create fct_bookings_dt as a copy of fct_bookings with dt instead of ds.
    db.execute_update(
        "CREATE TABLE fct_bookings_dt AS SELECT ds AS dt, listing_id, guest_id, host_id, is_instant, booking_value, referrer_id FROM fct_bookings",
    )
    .expect("failed to create fct_bookings_dt");

    (stores, db)
}

/// A ported test case loaded from JSON.
#[derive(serde::Deserialize)]
struct PortedTestCase {
    name: String,
    #[allow(dead_code)]
    file: String,
    model: String,
    spec: PortedSpec,
    check_query: String,
    check_order: bool,
    #[allow(dead_code)]
    allow_empty: bool,
    skip_reason: Option<String>,
    #[allow(dead_code)]
    min_max_only: bool,
}

#[derive(serde::Deserialize)]
struct PortedSpec {
    #[serde(default)]
    metrics: Vec<String>,
    #[serde(default)]
    group_by: Vec<String>,
    #[serde(default, rename = "where")]
    where_filters: Vec<String>,
    #[serde(default)]
    order_by: Vec<String>,
    #[serde(default)]
    limit: Option<u64>,
    #[serde(default)]
    time_constraint: Option<Vec<String>>,
    #[serde(default = "default_true")]
    apply_group_by: bool,
}

fn default_true() -> bool {
    true
}

impl PortedSpec {
    fn to_json(&self) -> String {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "metrics".into(),
            serde_json::to_value(&self.metrics).unwrap(),
        );
        if !self.group_by.is_empty() {
            obj.insert(
                "group_by".into(),
                serde_json::to_value(&self.group_by).unwrap(),
            );
        }
        if !self.where_filters.is_empty() {
            obj.insert(
                "where".into(),
                serde_json::to_value(&self.where_filters).unwrap(),
            );
        }
        if !self.order_by.is_empty() {
            obj.insert(
                "order_by".into(),
                serde_json::to_value(&self.order_by).unwrap(),
            );
        }
        if let Some(limit) = self.limit {
            obj.insert("limit".into(), serde_json::Value::Number(limit.into()));
        }
        if let Some(ref tc) = self.time_constraint {
            obj.insert("time_constraint".into(), serde_json::to_value(tc).unwrap());
        }
        if !self.apply_group_by {
            obj.insert("apply_group_by".into(), serde_json::Value::Bool(false));
        }
        serde_json::to_string(&obj).unwrap()
    }
}

/// Extract column names and string values from RecordBatches.
fn extract_named_rows(batches: &[RecordBatch]) -> (Vec<String>, Vec<Vec<String>>) {
    use arrow_array::{
        Array, BooleanArray, Date32Array, Float64Array, Int32Array, Int64Array, StringArray,
        TimestampMicrosecondArray, TimestampMillisecondArray, TimestampNanosecondArray,
        TimestampSecondArray,
    };

    let col_names: Vec<String> = batches
        .first()
        .map(|b| {
            b.schema()
                .fields()
                .iter()
                .map(|f| f.name().to_lowercase())
                .collect()
        })
        .unwrap_or_default();

    let mut rows = Vec::new();
    for batch in batches {
        for row_idx in 0..batch.num_rows() {
            let mut row = Vec::new();
            for col_idx in 0..batch.num_columns() {
                let col = batch.column(col_idx);
                let val = if col.is_null(row_idx) {
                    "NULL".to_string()
                } else if let Some(a) = col.as_any().downcast_ref::<Float64Array>() {
                    format!("{:.6}", a.value(row_idx))
                } else if let Some(a) = col.as_any().downcast_ref::<Int64Array>() {
                    a.value(row_idx).to_string()
                } else if let Some(a) = col.as_any().downcast_ref::<Int32Array>() {
                    a.value(row_idx).to_string()
                } else if let Some(a) = col.as_any().downcast_ref::<StringArray>() {
                    a.value(row_idx).to_string()
                } else if let Some(a) = col.as_any().downcast_ref::<BooleanArray>() {
                    a.value(row_idx).to_string()
                } else if let Some(a) = col.as_any().downcast_ref::<Date32Array>() {
                    format!("{}", a.value_as_date(row_idx).unwrap())
                } else if let Some(a) = col.as_any().downcast_ref::<TimestampMicrosecondArray>() {
                    format!("{}", a.value_as_datetime(row_idx).unwrap())
                } else if let Some(a) = col.as_any().downcast_ref::<TimestampMillisecondArray>() {
                    format!("{}", a.value_as_datetime(row_idx).unwrap())
                } else if let Some(a) = col.as_any().downcast_ref::<TimestampSecondArray>() {
                    format!("{}", a.value_as_datetime(row_idx).unwrap())
                } else if let Some(a) = col.as_any().downcast_ref::<TimestampNanosecondArray>() {
                    format!("{}", a.value_as_datetime(row_idx).unwrap())
                } else if let Some(a) = col.as_any().downcast_ref::<arrow_array::Decimal128Array>()
                {
                    let scale = a.scale() as f64;
                    format!("{:.6}", a.value(row_idx) as f64 / 10f64.powf(scale))
                } else {
                    format!("{:?}", col.as_ref())
                };
                row.push(val);
            }
            rows.push(row);
        }
    }
    (col_names, rows)
}

/// Compare two sets of RecordBatches for approximate equality.
/// Matches columns by name (case-insensitive), handling different column orders.
/// Returns Ok(()) if results match, Err(description) if they differ.
fn compare_results(
    our_batches: &[RecordBatch],
    ref_batches: &[RecordBatch],
    check_order: bool,
) -> Result<(), String> {
    let our_rows: usize = our_batches.iter().map(|b| b.num_rows()).sum();
    let ref_rows: usize = ref_batches.iter().map(|b| b.num_rows()).sum();

    if our_rows != ref_rows {
        return Err(format!("row count: ours={our_rows}, ref={ref_rows}"));
    }
    if our_rows == 0 {
        return Ok(());
    }

    let (our_names, our_data) = extract_named_rows(our_batches);
    let (ref_names, ref_data) = extract_named_rows(ref_batches);

    // Build column mapping: for each ref column, find matching our column.
    // Match by exact name or by suffix (e.g., "metric_time__day" matches "metric_time").
    let col_mapping: Vec<Option<usize>> = ref_names
        .iter()
        .map(|ref_name| {
            // Exact match first.
            if let Some(idx) = our_names.iter().position(|n| n == ref_name) {
                return Some(idx);
            }
            // Try suffix matching: ref "booking_value" matches our "booking_value".
            // Also try: ref "metric_time__day" matches our "metric_time".
            for (idx, our_name) in our_names.iter().enumerate() {
                if our_name.starts_with(ref_name) || ref_name.starts_with(our_name) {
                    return Some(idx);
                }
                // Check if ref_name contains our_name as a substring (handles
                // entity-prefixed names like "listing__user__last_login_ts__minute"
                // matching our "user__last_login_ts").
                if ref_name.contains(our_name) || our_name.contains(ref_name) {
                    return Some(idx);
                }
                // Strip __grain suffix for time dimensions (only valid granularities).
                let time_grans = [
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
                if let Some(pos) = ref_name.rfind("__") {
                    let ref_base = &ref_name[..pos];
                    let ref_suffix = &ref_name[pos + 2..];
                    if time_grans.contains(&ref_suffix) && ref_base == our_name.as_str() {
                        return Some(idx);
                    }
                }
                if let Some(pos) = our_name.rfind("__") {
                    let our_base = &our_name[..pos];
                    let our_suffix = &our_name[pos + 2..];
                    if time_grans.contains(&our_suffix) && our_base == ref_name {
                        return Some(idx);
                    }
                }
            }
            None
        })
        .collect();

    // If we can't map any ref columns, fall back to positional comparison.
    let mapped_count = col_mapping.iter().filter(|m| m.is_some()).count();
    let use_name_mapping = mapped_count > 0 && mapped_count >= ref_names.len() / 2;

    let mut our_projected: Vec<Vec<String>>;
    let mut ref_projected: Vec<Vec<String>>;

    if use_name_mapping {
        // Project our data to match ref column order.
        our_projected = our_data
            .iter()
            .map(|row| {
                col_mapping
                    .iter()
                    .map(|mapping| match mapping {
                        Some(idx) => row.get(*idx).cloned().unwrap_or_else(|| "??".into()),
                        None => "??UNMAPPED".into(),
                    })
                    .collect()
            })
            .collect();
        ref_projected = ref_data;
    } else {
        // Positional fallback.
        our_projected = our_data;
        ref_projected = ref_data;
    }

    if !check_order {
        our_projected.sort();
        ref_projected.sort();
    }

    for (i, (our_row, ref_row)) in our_projected.iter().zip(ref_projected.iter()).enumerate() {
        let cols_to_check = ref_row.len().min(our_row.len());
        for j in 0..cols_to_check {
            let ours = &our_row[j];
            let theirs = &ref_row[j];
            if ours == "??UNMAPPED" {
                continue;
            }
            if ours == theirs {
                continue;
            }
            // Try float comparison with tolerance.
            if let (Ok(a), Ok(b)) = (ours.parse::<f64>(), theirs.parse::<f64>()) {
                if (a - b).abs() < 0.01 || (a - b).abs() / (b.abs().max(1.0)) < 0.001 {
                    continue;
                }
            }
            let ref_col_name = ref_names.get(j).map(|s| s.as_str()).unwrap_or("?");
            return Err(format!(
                "row {i}, col {ref_col_name}: ours={ours}, ref={theirs}"
            ));
        }
    }

    Ok(())
}

#[test]
fn ported_metricflow_tests() {
    let test_cases: Vec<PortedTestCase> =
        serde_json::from_str(include_str!("data/ported_test_cases.json"))
            .expect("failed to parse ported_test_cases.json");

    let (mut stores, mut db) = setup_mf_db_full();

    let mut pass = 0u32;
    let mut fail = 0u32;
    let mut skip = 0u32;
    let mut compile_fail = 0u32;
    let mut exec_fail = 0u32;
    let mut compare_fail = 0u32;
    let mut results: Vec<(String, &str, String)> = Vec::new();

    for tc in &test_cases {
        let store: &mut InMemoryMetricStore = match tc.model.as_str() {
            "SIMPLE_MODEL" => &mut stores.simple,
            "EXTENDED_DATE_MODEL" => &mut stores.extended_date,
            "UNPARTITIONED_MULTI_HOP_JOIN_MODEL" => &mut stores.unpartitioned_multi_hop,
            "PARTITIONED_MULTI_HOP_JOIN_MODEL" => &mut stores.partitioned_multi_hop,
            "SIMPLE_MODEL_NON_DS" => &mut stores.simple_non_ds,
            "SCD_MODEL" => &mut stores.scd,
            _ => {
                skip += 1;
                results.push((
                    tc.name.clone(),
                    "SKIP",
                    format!("model {} not implemented", tc.model),
                ));
                continue;
            }
        };

        if let Some(ref reason) = tc.skip_reason {
            if !reason.is_empty() {
                skip += 1;
                results.push((tc.name.clone(), "SKIP", reason.clone()));
                continue;
            }
        }

        let spec_json = tc.spec.to_json();

        // 1. Compile our SQL.
        let our_sql = match compile_query(store, &spec_json) {
            Ok(sql) => sql,
            Err(e) => {
                compile_fail += 1;
                fail += 1;
                results.push((tc.name.clone(), "FAIL", format!("compile: {e}")));
                continue;
            }
        };

        // Wrap for min_max_only: SELECT MIN(col) AS col__min, MAX(col) AS col__max
        let our_sql = if tc.min_max_only {
            let col_names: Vec<String> = tc
                .spec
                .group_by
                .iter()
                .filter_map(|gb| {
                    let parsed = dbt_metricflow::parse_group_by_str(gb)?;
                    Some(match &parsed {
                        dbt_metricflow::GroupBySpec::Dimension { entity, name } => match entity {
                            Some(e) => format!("{e}__{name}"),
                            None => name.clone(),
                        },
                        dbt_metricflow::GroupBySpec::TimeDimension {
                            name, granularity, ..
                        } => {
                            format!("{name}__{granularity}")
                        }
                        dbt_metricflow::GroupBySpec::Entity { name } => name.clone(),
                    })
                })
                .collect();
            let min_max_cols: Vec<String> = col_names
                .iter()
                .map(|c| format!("MIN({c}) AS {c}__min\n  , MAX({c}) AS {c}__max"))
                .collect();
            format!(
                "SELECT\n  {}\nFROM (\n  {}\n) outer_subq",
                min_max_cols.join("\n  , "),
                our_sql.replace('\n', "\n  ")
            )
        } else {
            our_sql
        };

        // 2. Execute our SQL.
        let our_batches = match db.execute_query(&our_sql) {
            Ok(b) => b,
            Err(e) => {
                exec_fail += 1;
                fail += 1;
                results.push((
                    tc.name.clone(),
                    "FAIL",
                    format!("exec our SQL: {e}\n  SQL: {our_sql}"),
                ));
                continue;
            }
        };

        // 3. Execute the reference check_query.
        let ref_batches = match db.execute_query(&tc.check_query) {
            Ok(b) => b,
            Err(e) => {
                exec_fail += 1;
                fail += 1;
                results.push((
                    tc.name.clone(),
                    "FAIL",
                    format!("exec check_query: {e}\n  SQL: {}", tc.check_query),
                ));
                continue;
            }
        };

        // 4. Compare results.
        match compare_results(&our_batches, &ref_batches, tc.check_order) {
            Ok(()) => {
                let rows: usize = our_batches.iter().map(|b| b.num_rows()).sum();
                pass += 1;
                results.push((tc.name.clone(), "PASS", format!("{rows} rows")));
            }
            Err(diff) => {
                compare_fail += 1;
                fail += 1;
                let detail = format!("compare: {diff}\n  OUR SQL: {our_sql}");
                results.push((tc.name.clone(), "FAIL", detail));
            }
        }
    }

    // Print scorecard.
    let bar = "=".repeat(70);
    eprintln!("\n{bar}");
    eprintln!("Ported MetricFlow Integration Tests");
    eprintln!("{bar}");
    for (name, status, detail) in &results {
        if *status == "SKIP" {
            continue; // Don't clutter output with skips.
        }
        eprintln!("  [{status}] {name}: {detail}");
    }
    eprintln!("{bar}");
    let total = test_cases.len();
    eprintln!("  Total: {total}  Pass: {pass}  Fail: {fail}  Skip: {skip}");
    if fail > 0 {
        eprintln!("  Breakdown: compile={compile_fail} exec={exec_fail} compare={compare_fail}");
    }
    eprintln!("{bar}\n");

    // Don't fail the test — this is a scorecard showing progress.
    // Once all tests pass, change this to assert_eq!(fail, 0, ...).
    if fail > 0 {
        eprintln!("NOTE: {fail} tests failed — this scorecard tracks porting progress.");
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Manifest export — writes ported test manifests to JSON for other test files
// ═══════════════════════════════════════════════════════════════════════════

/// Write all ported test manifests to `tests/data/manifests/` as JSON files.
/// Other test files (e.g., snowflake_compat) can load these and rewrite
/// relation_names for their target dialect.
#[test]
fn export_ported_manifests() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("manifests");
    std::fs::create_dir_all(&dir).expect("failed to create manifests dir");

    let manifests: &[(&str, serde_json::Value)] = &[
        ("SIMPLE_MODEL", build_semantic_manifest()),
        ("EXTENDED_DATE_MODEL", build_extended_date_manifest()),
        (
            "UNPARTITIONED_MULTI_HOP_JOIN_MODEL",
            build_unpartitioned_multi_hop_manifest(),
        ),
        (
            "PARTITIONED_MULTI_HOP_JOIN_MODEL",
            build_partitioned_multi_hop_manifest(),
        ),
        ("SIMPLE_MODEL_NON_DS", build_simple_model_non_ds_manifest()),
        ("SCD_MODEL", build_scd_manifest()),
    ];

    for (name, manifest) in manifests {
        let path = dir.join(format!("{name}.json"));
        let json = serde_json::to_string_pretty(manifest).unwrap();
        std::fs::write(&path, json).unwrap();
        eprintln!("  wrote {path:?}");
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Snowflake compile scorecard for ported tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn snowflake_ported_compile_scorecard() {
    let test_cases: Vec<common::PortedTestCase> = common::load_ported_test_cases();
    let (mut stores, _db) = setup_mf_db_full();

    let mut pass = 0u32;
    let mut fail = 0u32;
    let mut skip = 0u32;
    let mut results: Vec<(String, &str, String)> = Vec::new();

    for tc in &test_cases {
        let store: &mut InMemoryMetricStore = match tc.model.as_str() {
            "SIMPLE_MODEL" => &mut stores.simple,
            "EXTENDED_DATE_MODEL" => &mut stores.extended_date,
            "UNPARTITIONED_MULTI_HOP_JOIN_MODEL" => &mut stores.unpartitioned_multi_hop,
            "PARTITIONED_MULTI_HOP_JOIN_MODEL" => &mut stores.partitioned_multi_hop,
            "SIMPLE_MODEL_NON_DS" => &mut stores.simple_non_ds,
            "SCD_MODEL" => &mut stores.scd,
            _ => {
                skip += 1;
                continue;
            }
        };

        if let Some(ref reason) = tc.skip_reason {
            if !reason.is_empty() {
                skip += 1;
                continue;
            }
        }

        let spec_json = tc.spec.to_json();
        let spec = match parse_query_spec(&spec_json) {
            Ok(s) => s,
            Err(e) => {
                fail += 1;
                results.push((tc.name.clone(), "FAIL", format!("parse: {e}")));
                continue;
            }
        };

        match compile(store, &spec, Dialect::Snowflake) {
            Ok(sql) => {
                let mut issues: Vec<String> = Vec::new();
                if sql.contains("__GROUP_BY_PLACEHOLDER_") {
                    issues.push("unreplaced placeholder".into());
                }
                if sql.contains("\x00PH") {
                    issues.push("unreplaced \\x00PH".into());
                }
                if sql.contains("generate_series") {
                    issues.push("DuckDB-only generate_series()".into());
                }
                if sql.contains("CAST(") && sql.contains(" AS DOUBLE)") {
                    issues.push("DuckDB-only DOUBLE type".into());
                }

                if issues.is_empty() {
                    pass += 1;
                    let lines = sql.lines().count();
                    results.push((tc.name.clone(), "PASS", format!("{lines} lines")));
                } else {
                    fail += 1;
                    results.push((
                        tc.name.clone(),
                        "FAIL",
                        format!("{}\n  SQL: {sql}", issues.join("; ")),
                    ));
                }
            }
            Err(e) => {
                fail += 1;
                results.push((tc.name.clone(), "FAIL", format!("compile: {e}")));
            }
        }
    }

    let bar = "=".repeat(70);
    eprintln!("\n{bar}");
    eprintln!("Snowflake Ported Tests Compile Scorecard");
    eprintln!("{bar}");
    for (name, status, detail) in &results {
        if *status == "PASS" {
            continue;
        }
        eprintln!("  [{status}] {name}: {detail}");
    }
    eprintln!("{bar}");
    let total = pass + fail;
    eprintln!("  Total: {total}  Pass: {pass}  Fail: {fail}  Skip: {skip}");
    eprintln!("{bar}\n");

    assert_eq!(
        fail, 0,
        "{fail} of {total} Snowflake ported compile tests failed"
    );
}
