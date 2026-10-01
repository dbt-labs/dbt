#![allow(dead_code)]

use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_ipc::reader::StreamReader;
use arrow_ipc::writer::{IpcWriteOptions, StreamWriter};
use arrow_schema::{ArrowError, Schema};
use dbt_metricflow::{Dialect, InMemoryMetricStore, compile, parse_query_spec};
use serde_json::json;

#[allow(clippy::too_many_arguments)]
pub fn sm(
    name: &str,
    alias: &str,
    database: &str,
    schema: &str,
    primary_entity: Option<&str>,
    default_agg_time_dim: Option<&str>,
    measures: serde_json::Value,
    dimensions: serde_json::Value,
    entities: serde_json::Value,
) -> serde_json::Value {
    let relation_name = format!("\"{database}\".\"{schema}\".\"{alias}\"");
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

// ── Measure / dimension / entity / metric helpers ────────────────────────
// (Identical to metricflow_compat.rs — copied for test isolation.)

pub fn measure(name: &str, agg: &str, expr: Option<&str>) -> serde_json::Value {
    json!({
        "name": name, "agg": agg, "expr": expr,
        "description": null, "label": null, "create_metric": false,
        "agg_time_dimension": null, "agg_params": null,
        "non_additive_dimension": null, "config": null,
    })
}

pub fn measure_non_additive(
    name: &str,
    agg: &str,
    expr: Option<&str>,
    nad_name: &str,
    window_choice: &str,
    window_groupings: Vec<&str>,
) -> serde_json::Value {
    let mut m = measure(name, agg, expr);
    m["non_additive_dimension"] = json!({
        "name": nad_name, "window_choice": window_choice, "window_groupings": window_groupings,
    });
    m
}

pub fn dim_categorical(name: &str, expr: Option<&str>) -> serde_json::Value {
    json!({
        "name": name, "type": "categorical", "description": null, "label": null,
        "expr": expr, "is_partition": false, "type_params": null,
        "metadata": null, "config": null,
    })
}

pub fn dim_time(name: &str, expr: Option<&str>, granularity: &str) -> serde_json::Value {
    json!({
        "name": name, "type": "time", "description": null, "label": null,
        "expr": expr, "is_partition": false,
        "type_params": { "time_granularity": granularity, "validity_params": null },
        "metadata": null, "config": null,
    })
}

pub fn dim_time_partition(name: &str, granularity: &str) -> serde_json::Value {
    json!({
        "name": name, "type": "time", "description": null, "label": null,
        "expr": null, "is_partition": true,
        "type_params": { "time_granularity": granularity, "validity_params": null },
        "metadata": null, "config": null,
    })
}

pub fn entity(name: &str, etype: &str, expr: &str) -> serde_json::Value {
    json!({
        "name": name, "type": etype, "description": null, "label": null,
        "role": null, "expr": expr, "config": null, "metadata": null,
    })
}

pub fn simple_metric(
    name: &str,
    _measure_name: &str,
    model: &str,
    agg: &str,
    expr: &str,
    agg_time_dim: &str,
) -> serde_json::Value {
    json!({
        "name": name, "description": name, "type": "simple",
        "label": null, "time_granularity": null, "filter": null,
        "metadata": null, "config": null,
        "type_params": {
            "measure": null, "input_measures": [], "numerator": null, "denominator": null,
            "expr": expr, "window": null, "grain_to_date": null, "metrics": null,
            "conversion_type_params": null, "cumulative_type_params": null,
            "join_to_timespine": false, "fill_nulls_with": null, "is_private": false,
            "metric_aggregation_params": {
                "semantic_model": model, "agg": agg, "agg_params": null,
                "agg_time_dimension": agg_time_dim, "non_additive_dimension": null, "expr": expr,
            },
        },
    })
}

pub fn simple_metric_with_filter(
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

pub fn derived_metric(
    name: &str,
    expr: &str,
    metrics: Vec<serde_json::Value>,
) -> serde_json::Value {
    json!({
        "name": name, "description": name, "type": "derived",
        "label": null, "time_granularity": null, "filter": null,
        "metadata": null, "config": null,
        "type_params": {
            "measure": null, "input_measures": [], "numerator": null, "denominator": null,
            "expr": expr, "window": null, "grain_to_date": null, "metrics": metrics,
            "conversion_type_params": null, "cumulative_type_params": null,
            "join_to_timespine": false, "fill_nulls_with": null, "is_private": false,
            "metric_aggregation_params": null,
        },
    })
}

pub fn ratio_metric(name: &str, numerator: &str, denominator: &str) -> serde_json::Value {
    json!({
        "name": name, "description": name, "type": "ratio",
        "label": null, "time_granularity": null, "filter": null,
        "metadata": null, "config": null,
        "type_params": {
            "measure": null, "input_measures": [],
            "numerator": { "name": numerator, "filter": null, "alias": null, "offset_window": null, "offset_to_grain": null },
            "denominator": { "name": denominator, "filter": null, "alias": null, "offset_window": null, "offset_to_grain": null },
            "expr": null, "window": null, "grain_to_date": null, "metrics": null,
            "conversion_type_params": null, "cumulative_type_params": null,
            "join_to_timespine": false, "fill_nulls_with": null, "is_private": false,
            "metric_aggregation_params": null,
        },
    })
}

pub fn cumulative_metric(
    name: &str,
    model: &str,
    agg: &str,
    expr: &str,
    agg_time_dim: &str,
    window: Option<serde_json::Value>,
    grain_to_date: Option<&str>,
) -> serde_json::Value {
    json!({
        "name": name, "description": name, "type": "cumulative",
        "label": null, "time_granularity": null, "filter": null,
        "metadata": null, "config": null,
        "type_params": {
            "measure": null, "input_measures": [], "numerator": null, "denominator": null,
            "expr": null, "window": null, "grain_to_date": null, "metrics": null,
            "conversion_type_params": null,
            "cumulative_type_params": {
                "window": window, "grain_to_date": grain_to_date,
            },
            "join_to_timespine": false, "fill_nulls_with": null, "is_private": false,
            "metric_aggregation_params": {
                "semantic_model": model, "agg": agg, "agg_params": null,
                "agg_time_dimension": agg_time_dim, "non_additive_dimension": null, "expr": expr,
            },
        },
    })
}

pub fn conversion_metric(
    name: &str,
    base_metric: &str,
    conversion_metric_name: &str,
    entity: &str,
    calc: &str,
    window: Option<serde_json::Value>,
) -> serde_json::Value {
    json!({
        "name": name, "description": name, "type": "conversion",
        "label": null, "time_granularity": null, "filter": null,
        "metadata": null, "config": null,
        "type_params": {
            "measure": null, "input_measures": [], "numerator": null, "denominator": null,
            "expr": null, "window": null, "grain_to_date": null, "metrics": null,
            "conversion_type_params": {
                "base_measure": { "name": base_metric, "filter": null, "alias": null, "offset_window": null, "offset_to_grain": null },
                "conversion_measure": { "name": conversion_metric_name, "filter": null, "alias": null, "offset_window": null, "offset_to_grain": null },
                "entity": entity, "calculation": calc, "window": window,
                "constant_properties": null,
            },
            "cumulative_type_params": null,
            "join_to_timespine": false, "fill_nulls_with": null, "is_private": false,
            "metric_aggregation_params": null,
        },
    })
}

pub fn metric_input(name: &str) -> serde_json::Value {
    json!({ "name": name, "filter": null, "alias": null, "offset_window": null, "offset_to_grain": null })
}

pub fn metric_input_alias(name: &str, alias: &str) -> serde_json::Value {
    json!({ "name": name, "filter": null, "alias": alias, "offset_window": null, "offset_to_grain": null })
}

pub fn metric_input_offset_window(
    name: &str,
    alias: &str,
    offset_window: &str,
) -> serde_json::Value {
    json!({
        "name": name, "filter": null, "alias": alias,
        "offset_window": offset_window, "offset_to_grain": null,
    })
}

pub fn metric_input_offset_to_grain(
    name: &str,
    alias: &str,
    offset_to_grain: &str,
) -> serde_json::Value {
    json!({
        "name": name, "filter": null, "alias": alias,
        "offset_window": null, "offset_to_grain": offset_to_grain,
    })
}

pub fn measure_with_agg_time(
    name: &str,
    agg: &str,
    expr: Option<&str>,
    agg_time_dim: &str,
) -> serde_json::Value {
    let mut m = measure(name, agg, expr);
    m["agg_time_dimension"] = json!(agg_time_dim);
    m
}

pub fn metric_input_with_filter(name: &str, alias: &str, filter: &str) -> serde_json::Value {
    json!({
        "name": name,
        "filter": { "where_filters": [{"where_sql_template": filter}] },
        "alias": alias,
        "offset_window": null,
        "offset_to_grain": null,
    })
}

pub fn simple_metric_filtered(
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

pub fn ratio_metric_with_filter(
    name: &str,
    numerator: &str,
    denominator: &str,
    filter: &str,
) -> serde_json::Value {
    let mut m = ratio_metric(name, numerator, denominator);
    m["filter"] = json!({"where_filters": [{"where_sql_template": filter}]});
    m
}

pub fn simple_metric_with_spine(
    name: &str,
    model: &str,
    agg: &str,
    expr: &str,
    agg_time_dim: &str,
    join_to_timespine: bool,
    fill_nulls_with: Option<i64>,
) -> serde_json::Value {
    json!({
        "name": name, "description": name, "type": "simple",
        "label": null, "time_granularity": null, "filter": null,
        "metadata": null, "config": null,
        "type_params": {
            "measure": null, "input_measures": [], "numerator": null, "denominator": null,
            "expr": expr, "window": null, "grain_to_date": null, "metrics": null,
            "conversion_type_params": null, "cumulative_type_params": null,
            "join_to_timespine": join_to_timespine, "fill_nulls_with": fill_nulls_with,
            "is_private": false,
            "metric_aggregation_params": {
                "semantic_model": model, "agg": agg, "agg_params": null,
                "agg_time_dimension": agg_time_dim, "non_additive_dimension": null, "expr": expr,
            },
        },
    })
}

pub fn conversion_metric_with_const_props(
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
        "name": name, "description": name, "type": "conversion",
        "label": null, "time_granularity": null, "filter": null,
        "metadata": null, "config": null,
        "type_params": {
            "measure": null, "input_measures": [], "numerator": null, "denominator": null,
            "expr": null, "window": null, "grain_to_date": null, "metrics": null,
            "conversion_type_params": {
                "base_measure": { "name": base_measure },
                "conversion_measure": { "name": conversion_measure },
                "entity": entity, "calculation": calculation, "window": window,
                "constant_properties": props,
            },
            "cumulative_type_params": null,
            "join_to_timespine": false, "fill_nulls_with": null, "is_private": false,
            "metric_aggregation_params": null,
        },
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// Manifest + DDL builders (parameterized by database + schema)
// ═══════════════════════════════════════════════════════════════════════════

pub fn build_semantic_manifest(database: &str, schema: &str) -> serde_json::Value {
    let bookings_source = sm(
        "bookings_source",
        "FCT_BOOKINGS",
        database,
        schema,
        Some("booking"),
        Some("ds"),
        json!([
            measure("bookings", "sum", Some("1")),
            measure("instant_bookings", "sum_boolean", Some("is_instant")),
            measure("booking_value", "sum", Some("booking_value")),
            measure("max_booking_value", "max", Some("booking_value")),
            measure("min_booking_value", "min", Some("booking_value")),
            measure("bookers", "count_distinct", Some("guest_id")),
            measure("average_booking_value", "average", Some("booking_value")),
            measure_with_agg_time("booking_payments", "sum", Some("booking_value"), "paid_at"),
            measure("referred_bookings", "count", Some("referrer_id")),
            measure("median_booking_value", "median", Some("booking_value")),
        ]),
        json!([
            dim_time("ds", None, "day"),
            dim_time("paid_at", None, "day"),
            dim_time_partition("ds_partitioned", "day"),
            dim_categorical("is_instant", Some("is_instant")),
        ]),
        json!([
            entity("booking", "primary", "1"),
            entity("listing", "foreign", "listing_id"),
            entity("guest", "foreign", "guest_id"),
            entity("host", "foreign", "host_id"),
        ]),
    );

    let listings_latest = sm(
        "listings_latest",
        "DIM_LISTINGS_LATEST",
        database,
        schema,
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
        "FCT_VIEWS",
        database,
        schema,
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
        "DIM_USERS_LATEST",
        database,
        schema,
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
        "DIM_USERS",
        database,
        schema,
        None,
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
        ]),
        json!([entity("user", "primary", "user_id"),]),
    );

    let revenue = sm(
        "revenue",
        "FCT_REVENUE",
        database,
        schema,
        Some("revenue_instance"),
        Some("ds"),
        json!([measure("txn_revenue", "sum", Some("revenue")),]),
        json!([dim_time("ds", Some("created_at"), "day"),]),
        json!([entity("user", "foreign", "user_id"),]),
    );

    let accounts_source = sm(
        "accounts_source",
        "FCT_ACCOUNTS",
        database,
        schema,
        Some("account"),
        Some("ds"),
        json!([
            measure("account_balance", "sum", None),
            measure_non_additive(
                "total_account_balance_first_day",
                "sum",
                Some("account_balance"),
                "ds",
                "min",
                vec![]
            ),
            measure_non_additive(
                "current_account_balance_by_user",
                "sum",
                Some("account_balance"),
                "ds",
                "max",
                vec!["user"]
            ),
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
        "FCT_ID_VERIFICATIONS",
        database,
        schema,
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
        "FCT_VISITS",
        database,
        schema,
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
        "FCT_BUYS",
        database,
        schema,
        Some("buy"),
        Some("ds"),
        json!([measure("buys", "count", Some("1")),]),
        json!([dim_time("ds", None, "day"),]),
        json!([entity("user", "foreign", "user_id"),]),
    );

    let companies = sm(
        "companies",
        "DIM_COMPANIES",
        database,
        schema,
        None,
        None,
        json!([]),
        json!([dim_categorical("company_name", None),]),
        json!([
            entity("company", "primary", "company_id"),
            entity("user", "unique", "user_id"),
        ]),
    );

    let lux_listing_mapping = sm(
        "lux_listing_mapping",
        "DIM_LUX_LISTING_ID_MAPPING",
        database,
        schema,
        None,
        None,
        json!([]),
        json!([]),
        json!([
            entity("listing", "primary", "listing_id"),
            entity("lux_listing", "foreign", "lux_listing_id"),
        ]),
    );

    let company_regions = sm(
        "company_regions",
        "DIM_COMPANY_REGIONS",
        database,
        schema,
        None,
        None,
        json!([]),
        json!([dim_categorical("region_name", None),]),
        json!([
            entity("region", "primary", "region_id"),
            entity("company", "foreign", "company_id"),
        ]),
    );

    let conversions_source = sm(
        "conversions_source",
        "FCT_CONVERSIONS",
        database,
        schema,
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
        "FCT_SIGNUPS",
        database,
        schema,
        Some("signup_account"),
        Some("created_at"),
        json!([measure("signups", "sum", Some("1")),]),
        json!([
            dim_time("created_at", None, "day"),
            dim_categorical("source", None),
        ]),
        json!([]),
    );

    // Semi-additive metrics
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
            vec![metric_input("booking_value"), metric_input("bookers")]
        ),
        derived_metric(
            "views_times_booking_value",
            "booking_value * views",
            vec![metric_input("booking_value"), metric_input("views")]
        ),
        derived_metric(
            "non_referred_bookings_pct",
            "(bookings - ref_bookings) * 1.0 / bookings",
            vec![
                metric_input_alias("referred_bookings", "ref_bookings"),
                metric_input("bookings")
            ]
        ),
        derived_metric(
            "booking_value_sub_instant",
            "booking_value - instant_booking_value",
            vec![
                metric_input("instant_booking_value"),
                metric_input("booking_value")
            ]
        ),
        derived_metric(
            "booking_value_sub_instant_add_10",
            "booking_value_sub_instant + 10",
            vec![metric_input("booking_value_sub_instant")]
        ),
        // Offset window derived metrics
        derived_metric(
            "bookings_growth_1_day",
            "bookings - bookings_1_day_ago",
            vec![
                metric_input("bookings"),
                metric_input_offset_window("bookings", "bookings_1_day_ago", "1 day")
            ]
        ),
        // Offset to grain derived metrics
        derived_metric(
            "bookings_growth_since_start_of_month",
            "bookings - bookings_at_start_of_month",
            vec![
                metric_input("bookings"),
                metric_input_offset_to_grain("bookings", "bookings_at_start_of_month", "month")
            ]
        ),
        // Ratio metrics
        ratio_metric("bookings_per_booker", "bookings", "bookers"),
        ratio_metric("bookings_per_view", "bookings", "views"),
        ratio_metric("bookings_per_listing", "bookings", "listings"),
        ratio_metric("bookings_per_dollar", "bookings", "booking_value"),
        // Cumulative metrics
        cumulative_metric(
            "trailing_2_months_revenue",
            "revenue",
            "sum",
            "revenue",
            "ds",
            Some(json!({"count": 2, "granularity": "month"})),
            None
        ),
        cumulative_metric(
            "revenue_all_time",
            "revenue",
            "sum",
            "revenue",
            "ds",
            None,
            None
        ),
        cumulative_metric(
            "revenue_mtd",
            "revenue",
            "sum",
            "revenue",
            "ds",
            None,
            Some("month")
        ),
        cumulative_metric(
            "cumulative_bookings",
            "bookings_source",
            "sum",
            "1",
            "ds",
            None,
            None
        ),
        cumulative_metric(
            "every_2_days_bookers",
            "bookings_source",
            "count_distinct",
            "guest_id",
            "ds",
            Some(json!({"count": 2, "granularity": "day"})),
            None
        ),
        // Time spine / fill_nulls metrics
        simple_metric_with_spine(
            "bookings_fill_nulls_no_spine",
            "bookings_source",
            "sum",
            "1",
            "ds",
            false,
            Some(0)
        ),
        simple_metric_with_spine(
            "bookings_join_to_time_spine",
            "bookings_source",
            "sum",
            "1",
            "ds",
            true,
            None
        ),
        simple_metric_with_spine(
            "bookings_fill_nulls_with_spine",
            "bookings_source",
            "sum",
            "1",
            "ds",
            true,
            Some(0)
        ),
        simple_metric_with_spine(
            "revenue_fill_nulls_with_spine",
            "revenue",
            "sum",
            "revenue",
            "ds",
            true,
            Some(0)
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
            Some(json!({"count": 7, "granularity": "day"}))
        ),
        conversion_metric(
            "visit_buy_conversions_7days",
            "visits",
            "buys",
            "user",
            "conversions",
            Some(json!({"count": 7, "granularity": "day"}))
        ),
        conversion_metric(
            "visit_buy_conversion_rate_unbounded",
            "visits",
            "buys",
            "user",
            "conversion_rate",
            None
        ),
        conversion_metric_with_const_props(
            "visit_buy_conversion_rate_by_session",
            "visits",
            "buys",
            "user",
            "conversion_rate",
            Some(json!({"count": 7, "granularity": "day"})),
            vec![("session_id", "session_id")],
        ),
        // Semi-additive metrics
        semi_additive_first_day,
        semi_additive_current_by_user,
        // Derived metrics with shared alias names
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
        simple_metric_filtered(
            "booking_value_for_non_null_listing_id",
            "bookings_source",
            "sum",
            "booking_value",
            "ds",
            "{{ Entity('listing') }} IS NOT NULL",
        ),
        simple_metric_filtered(
            "lux_listings",
            "listings_latest",
            "count",
            "1",
            "ds",
            "{{ Dimension('listing__is_lux_latest') }}",
        ),
        derived_metric(
            "booking_value_per_view",
            "booking_value / NULLIF(views, 0)",
            vec![metric_input("booking_value"), metric_input("views")]
        ),
        derived_metric(
            "bookings_growth_2_weeks",
            "bookings - bookings_2_weeks_ago",
            vec![
                metric_input("bookings"),
                metric_input_offset_window("bookings", "bookings_2_weeks_ago", "14 days"),
            ]
        ),
        derived_metric(
            "bookings_5_day_lag",
            "bookings_5_days_ago",
            vec![metric_input_offset_window(
                "bookings",
                "bookings_5_days_ago",
                "5 days"
            ),]
        ),
        derived_metric(
            "bookings_month_start_compared_to_1_month_prior",
            "month_start_bookings - bookings_1_month_ago",
            vec![
                metric_input_offset_to_grain("bookings", "month_start_bookings", "month"),
                metric_input_offset_window("bookings", "bookings_1_month_ago", "1 month"),
            ]
        ),
        derived_metric(
            "instant_plus_non_referred_bookings_pct",
            "non_referred + (instant * 1.0 / bookings)",
            vec![
                metric_input_alias("non_referred_bookings_pct", "non_referred"),
                metric_input_alias("instant_bookings", "instant"),
                metric_input("bookings"),
            ]
        ),
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
        derived_metric(
            "every_2_days_bookers_2_days_ago",
            "every_2_days_bookers_2_days_ago",
            vec![metric_input_offset_window(
                "every_2_days_bookers",
                "every_2_days_bookers_2_days_ago",
                "2 days"
            ),]
        ),
        simple_metric_with_spine(
            "bookings_fill_nulls_with_0",
            "bookings_source",
            "sum",
            "1",
            "ds",
            true,
            Some(0),
        ),
        simple_metric_with_spine(
            "bookings_fill_nulls_with_0_without_time_spine",
            "bookings_source",
            "sum",
            "1",
            "ds",
            false,
            Some(0),
        ),
        cumul_fill_nulls,
        derived_metric(
            "bookings_growth_2_weeks_fill_nulls_with_0_for_non_offset",
            "bookings_fill_nulls_with_0 - bookings_2_weeks_ago",
            vec![
                metric_input("bookings_fill_nulls_with_0"),
                metric_input_offset_window("bookings", "bookings_2_weeks_ago", "14 days"),
            ]
        ),
        derived_metric(
            "bookings_offset_once",
            "bookings_1_day_ago",
            vec![metric_input_offset_window(
                "bookings",
                "bookings_1_day_ago",
                "5 days"
            ),]
        ),
        derived_metric(
            "bookings_offset_twice",
            "2 * bookings_offset_once",
            vec![metric_input_offset_window(
                "bookings_offset_once",
                "bookings_offset_once",
                "2 days"
            ),]
        ),
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
        simple_metric_with_filter(
            "active_listings",
            "listings",
            "listings_latest",
            "count",
            "1",
            "ds",
            json!({"where_filters": [{"where_sql_template": "{{ Metric('bookings', ['listing']) }} > 2"}]}),
        ),
        ratio_metric_with_filter(
            "popular_listing_bookings_per_booker",
            "listings",
            "listings",
            "{{ Metric('views', ['listing']) }} > 10",
        ),
        // Metrics for bare dimension name collision test
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
        simple_metric(
            "archived_users",
            "archived_users",
            "users_ds_source",
            "sum",
            "1",
            "archived_at"
        ),
        simple_metric_with_spine(
            "subdaily_join_to_time_spine_metric",
            "users_ds_source",
            "sum",
            "1",
            "archived_at",
            true,
            None,
        ),
        derived_metric(
            "subdaily_offset_window_metric",
            "archived_users - archived_users_1_day_ago",
            vec![
                metric_input("archived_users"),
                metric_input_offset_window("archived_users", "archived_users_1_day_ago", "1 day"),
            ]
        ),
        derived_metric(
            "subdaily_offset_grain_to_date_metric",
            "archived_users - archived_users_at_start_of_month",
            vec![
                metric_input("archived_users"),
                metric_input_offset_to_grain(
                    "archived_users",
                    "archived_users_at_start_of_month",
                    "month"
                ),
            ]
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
        derived_metric(
            "fill_nulls_derived_test",
            "bookings_fill_nulls_with_0 + views",
            vec![
                metric_input("bookings_fill_nulls_with_0"),
                metric_input("views"),
            ]
        ),
    ]);

    let project_configuration = json!({
        "time_spine_table_configurations": [],
        "metadata": null,
        "dsi_package_version": { "major_version": "0", "minor_version": "0", "patch_version": "0" },
        "time_spines": [
            {
                "node_relation": { "alias": "MF_TIME_SPINE", "schema_name": schema, "database": database, "relation_name": null },
                "primary_column": { "name": "ds", "time_granularity": "day" },
                "custom_granularities": [],
            },
        ],
    });

    let saved_queries = json!([
        {
            "name": "p0_booking",
            "description": "Booking-related metrics.",
            "label": null, "metadata": null, "tags": [],
            "query_params": {
                "metrics": ["bookings", "instant_bookings"],
                "group_by": ["TimeDimension('metric_time', 'day')", "Dimension('listing__capacity_latest')"],
                "where": ["{{ Dimension('listing__capacity_latest') }} > 3"],
                "order_by": [], "limit": null,
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

pub fn snowflake_data_ddl(schema: &str) -> Vec<String> {
    let mut stmts = Vec::new();

    stmts.push(format!(
        "CREATE TABLE {schema}.fct_bookings (
        ds DATE, ds_partitioned DATE, paid_at DATE,
        guest_id VARCHAR, host_id VARCHAR, listing_id VARCHAR,
        booking_value DOUBLE, is_instant BOOLEAN, referrer_id VARCHAR
    )"
    ));

    stmts.push(format!("INSERT INTO {schema}.fct_bookings VALUES
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
    "));

    stmts.push(format!(
        "CREATE TABLE {schema}.dim_listings_latest (
        created_at DATE, listing_id VARCHAR, country VARCHAR,
        capacity INTEGER, is_lux BOOLEAN, user_id VARCHAR
    )"
    ));
    stmts.push(format!(
        "INSERT INTO {schema}.dim_listings_latest VALUES
        ('2020-01-01','l3141592','us',3,true,'u0004114'),
        ('2020-01-02','l5948301','us',5,true,'u0004114'),
        ('2020-01-02','l2718281','cote d''ivoire',4,false,'u0005432'),
        ('2020-01-02','l9658588-incomplete','us',null,null,'u1004114'),
        ('2020-01-02','l8912456-incomplete',null,null,null,'u1004114'),
        ('2020-01-02','l7891283-incomplete','ca',null,false,'u1004114')
    "
    ));

    stmts.push(format!(
        "CREATE TABLE {schema}.fct_views (
        ds DATE, ds_partitioned DATE, listing_id VARCHAR, user_id VARCHAR
    )"
    ));
    stmts.push(format!(
        "INSERT INTO {schema}.fct_views VALUES
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
    ));

    stmts.push(format!(
        "CREATE TABLE {schema}.dim_users_latest (ds DATE, user_id VARCHAR, home_state_latest VARCHAR)"
    ));
    stmts.push(format!(
        "INSERT INTO {schema}.dim_users_latest VALUES
        ('2020-01-01','u0003141','MD'),('2020-01-01','u0003154','CA'),
        ('2020-01-01','u0003452','HI'),('2020-01-01','u0004114','NY'),
        ('2020-01-01','u0005432','WA'),('2020-01-01','u1004114','TX'),
        ('2020-01-01','u1003452','CA')
    "
    ));

    stmts.push(format!(
        "CREATE TABLE {schema}.dim_users (
        ds DATE, ds_partitioned DATE, created_at DATE, user_id VARCHAR,
        home_state VARCHAR, last_profile_edit_ts TIMESTAMP_NTZ,
        bio_added_ts TIMESTAMP_NTZ, last_login_ts TIMESTAMP_NTZ, archived_at TIMESTAMP_NTZ
    )"
    ));
    stmts.push(format!(
        "INSERT INTO {schema}.dim_users VALUES
        ('2019-12-19','2019-12-19','2019-12-19','u0003452','HI','2019-12-19 10:30:00','2019-12-19 09:00:00','2019-12-19 08:15:00','2019-12-19 10:00:00'),
        ('2019-12-19','2019-12-19','2019-12-19','u0004114','NY','2019-12-19 11:00:00','2019-12-19 10:00:00','2019-12-19 09:30:00','2019-12-19 10:00:00'),
        ('2019-12-19','2019-12-19','2019-12-19','u0003141','MD','2019-12-19 12:30:00','2019-12-19 11:00:00','2019-12-19 10:45:00','2019-12-19 11:00:00'),
        ('2019-12-19','2019-12-19','2019-12-19','u0003154','CA','2019-12-19 14:00:00','2019-12-19 13:00:00','2019-12-19 12:00:00','2019-12-19 11:00:00'),
        ('2019-12-19','2019-12-19','2019-12-19','u0005432','WA','2019-12-19 15:30:00','2019-12-19 14:00:00','2019-12-19 13:15:00','2019-12-19 12:00:00'),
        ('2019-12-19','2019-12-19','2019-12-19','u1004114','TX','2019-12-19 16:00:00','2019-12-19 15:00:00','2019-12-19 14:30:00','2019-12-19 12:00:00'),
        ('2019-12-19','2019-12-19','2019-12-19','u1003452','CA','2019-12-19 17:30:00','2019-12-19 16:00:00','2019-12-19 15:45:00','2019-12-19 13:00:00')
    "
    ));

    stmts.push(format!(
        "CREATE TABLE {schema}.fct_revenue (created_at DATE, user_id VARCHAR, revenue DOUBLE)"
    ));
    stmts.push(format!(
        "INSERT INTO {schema}.fct_revenue VALUES
        ('2019-12-19','u0004114',100.0),('2019-12-19','u0003141',200.0),
        ('2019-12-20','u0004114',150.0),('2019-12-20','u0003452',50.0),
        ('2020-01-01','u0004114',300.0),('2020-01-02','u0005432',75.0)
    "
    ));

    stmts.push(format!(
        "CREATE TABLE {schema}.fct_accounts (
        ds DATE, user_id VARCHAR, account_balance DOUBLE,
        account_type VARCHAR, ds_month DATE
    )"
    ));
    stmts.push(format!(
        "INSERT INTO {schema}.fct_accounts VALUES
        ('2019-12-19','u0003141',1000.0,'checking','2019-12-01'),
        ('2019-12-19','u0004114',2000.0,'savings','2019-12-01'),
        ('2019-12-20','u0003141',1100.0,'checking','2019-12-01'),
        ('2019-12-20','u0004114',1800.0,'savings','2019-12-01'),
        ('2020-01-01','u0003141',1200.0,'checking','2020-01-01'),
        ('2020-01-01','u0004114',1900.0,'savings','2020-01-01')
    "
    ));

    stmts.push(format!(
        "CREATE TABLE {schema}.fct_id_verifications (
        ds DATE, ds_partitioned DATE, verification_id VARCHAR,
        user_id VARCHAR, verification_type VARCHAR
    )"
    ));
    stmts.push(format!(
        "INSERT INTO {schema}.fct_id_verifications VALUES
        ('2019-12-19','2019-12-19','v001','u0003141','phone'),
        ('2019-12-19','2019-12-19','v002','u0004114','id_card'),
        ('2019-12-20','2019-12-20','v003','u0003452','phone'),
        ('2020-01-01','2020-01-01','v004','u0005432','passport')
    "
    ));

    stmts.push(format!(
        "CREATE TABLE {schema}.fct_visits (ds DATE, user_id VARCHAR, session_id VARCHAR, referrer_id VARCHAR)"
    ));
    stmts.push(format!(
        "INSERT INTO {schema}.fct_visits VALUES
        ('2019-12-19','u0003141','s001','fb_ad_1'),
        ('2019-12-19','u0004114','s002','google'),
        ('2019-12-19','u0003452','s003',null),
        ('2019-12-20','u0003141','s004','fb_ad_1'),
        ('2019-12-20','u0005432','s005','email')
    "
    ));

    stmts.push(format!(
        "CREATE TABLE {schema}.fct_buys (ds DATE, user_id VARCHAR, session_id VARCHAR, ds_month DATE)"
    ));
    stmts.push(format!(
        "INSERT INTO {schema}.fct_buys VALUES
        ('2019-12-19','u0003141','s001','2019-12-01'),
        ('2019-12-20','u0003141','s004','2019-12-01'),
        ('2019-12-20','u0005432','s005','2019-12-01')
    "
    ));

    stmts.push(format!(
        "CREATE TABLE {schema}.dim_companies (company_id VARCHAR, user_id VARCHAR, company_name VARCHAR)"
    ));
    stmts.push(format!(
        "INSERT INTO {schema}.dim_companies VALUES ('c001','u0003141','Acme Corp'),('c002','u0004114','Globex Inc')"
    ));

    stmts.push(format!(
        "CREATE TABLE {schema}.dim_lux_listing_id_mapping (listing_id VARCHAR, lux_listing_id VARCHAR)"
    ));
    stmts.push(format!(
        "INSERT INTO {schema}.dim_lux_listing_id_mapping VALUES ('l3141592','lux_001'),('l5948301','lux_002')"
    ));

    stmts.push(format!(
        "CREATE TABLE {schema}.dim_company_regions (region_id VARCHAR, company_id VARCHAR, region_name VARCHAR)"
    ));
    stmts.push(format!(
        "INSERT INTO {schema}.dim_company_regions VALUES ('r001','c001','Northeast'),('r002','c002','West Coast')"
    ));

    stmts.push(format!(
        "CREATE TABLE {schema}.fct_conversions (created_at DATE, channel VARCHAR)"
    ));
    stmts.push(format!(
        "INSERT INTO {schema}.fct_conversions VALUES ('2024-01-01','organic'),('2024-01-02','paid')"
    ));

    stmts.push(format!(
        "CREATE TABLE {schema}.fct_signups (created_at DATE, source VARCHAR)"
    ));
    stmts.push(format!(
        "INSERT INTO {schema}.fct_signups VALUES ('2024-01-01','web'),('2024-01-02','mobile')"
    ));

    stmts
}

pub fn redshift_data_ddl(schema: &str) -> Vec<String> {
    snowflake_data_ddl(schema)
        .into_iter()
        .map(|s| {
            let s = s
                .replace("DOUBLE,", "DOUBLE PRECISION,")
                .replace("DOUBLE)", "DOUBLE PRECISION)")
                .replace("TIMESTAMP_NTZ", "TIMESTAMP");
            quote_table_names_for_redshift(&s, schema)
        })
        .collect()
}

fn quote_table_names_for_redshift(sql: &str, schema: &str) -> String {
    let tables = [
        "fct_bookings",
        "dim_listings_latest",
        "fct_views",
        "dim_users_latest",
        "dim_users",
        "fct_revenue",
        "fct_accounts",
        "fct_id_verifications",
        "fct_visits",
        "fct_buys",
        "dim_companies",
        "dim_lux_listing_id_mapping",
        "dim_company_regions",
        "fct_conversions",
        "fct_signups",
    ];
    let mut result = sql.to_string();
    for table in tables {
        let unquoted = format!("{schema}.{table}");
        let quoted = format!("{schema}.\"{}\"", table.to_uppercase());
        result = result.replace(&unquoted, &quoted);
    }
    result
}

pub fn bigquery_data_ddl(dataset: &str) -> Vec<String> {
    let prefix = format!("{dataset}.");
    snowflake_data_ddl(dataset)
        .into_iter()
        .map(|s| {
            let s = s
                .replace("VARCHAR", "STRING")
                .replace("DOUBLE", "FLOAT64")
                .replace("INTEGER", "INT64")
                .replace("BOOLEAN", "BOOL")
                .replace("TIMESTAMP_NTZ", "TIMESTAMP");
            // Snowflake auto-uppercases unquoted identifiers; BigQuery is case-sensitive.
            // Uppercase table names after the dataset prefix to match manifest aliases.
            let mut result = String::new();
            let mut rest = s.as_str();
            while let Some(pos) = rest.find(&prefix) {
                result.push_str(&rest[..pos + prefix.len()]);
                rest = &rest[pos + prefix.len()..];
                let end = rest
                    .find(|c: char| !c.is_alphanumeric() && c != '_')
                    .unwrap_or(rest.len());
                result.push_str(&rest[..end].to_uppercase());
                rest = &rest[end..];
            }
            result.push_str(rest);
            result
        })
        .collect()
}

pub fn databricks_data_ddl(schema: &str) -> Vec<String> {
    snowflake_data_ddl(schema)
        .into_iter()
        .map(|s| {
            s.replace("VARCHAR", "STRING")
                .replace("INTEGER", "INT")
                .replace("TIMESTAMP_NTZ", "TIMESTAMP")
        })
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════════
// Test cases (same as metricflow_compat.rs)
// ═══════════════════════════════════════════════════════════════════════════

pub struct TestCase {
    pub name: &'static str,
    pub spec: &'static str,
    pub min_rows: usize,
    pub expected_value: Option<f64>,
}

pub const TEST_CASES: &[TestCase] = &[
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
    TestCase {
        name: "offset_window_bookings_growth_by_day",
        spec: r#"{"metrics": ["bookings_growth_1_day"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
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
    // ── Metric with filter ───────────────────────────────────────────
    TestCase {
        name: "metric_with_filter_instant_booking_value",
        spec: r#"{"metrics": ["instant_booking_value"]}"#,
        min_rows: 1,
        expected_value: None,
    },
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
    TestCase {
        name: "semi_additive_first_day_balance",
        spec: r#"{"metrics": ["total_account_balance_first_day"]}"#,
        min_rows: 1,
        expected_value: Some(3000.0),
    },
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
    TestCase {
        name: "multi_hop_bookings_by_company",
        spec: r#"{"metrics": ["bookings"], "group_by": ["Dimension('company__company_name')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "multi_hop_bookings_by_lux_listing",
        spec: r#"{"metrics": ["bookings"], "group_by": ["Entity('lux_listing')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "multi_hop_revenue_by_company",
        spec: r#"{"metrics": ["revenue"], "group_by": ["Dimension('company__company_name')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Conversion metrics ─────────────────────────────────────────
    TestCase {
        name: "conversion_rate_7day_window",
        spec: r#"{"metrics": ["visit_buy_conversion_rate_7days"]}"#,
        min_rows: 1,
        expected_value: Some(0.6),
    },
    TestCase {
        name: "conversion_count_7day_window",
        spec: r#"{"metrics": ["visit_buy_conversions_7days"]}"#,
        min_rows: 1,
        expected_value: Some(3.0),
    },
    TestCase {
        name: "conversion_rate_unbounded",
        spec: r#"{"metrics": ["visit_buy_conversion_rate_unbounded"]}"#,
        min_rows: 1,
        expected_value: Some(0.6),
    },
    TestCase {
        name: "conversion_rate_with_constant_property",
        spec: r#"{"metrics": ["visit_buy_conversion_rate_by_session"]}"#,
        min_rows: 1,
        expected_value: Some(0.6),
    },
    // ── WHERE filter with TimeDimension Jinja syntax ────────────────
    TestCase {
        name: "where_time_dimension_jinja",
        spec: r#"{"metrics": ["bookings"], "group_by": ["TimeDimension('metric_time', 'day')"], "where": ["{{ TimeDimension('metric_time', 'day') }} >= '2019-12-19'"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── WHERE filter with time range (two conditions) ───────────────
    TestCase {
        name: "where_time_range",
        spec: r#"{"metrics": ["bookings"], "group_by": ["TimeDimension('metric_time', 'day')"], "where": ["{{ TimeDimension('metric_time', 'day') }} >= '2019-12-18'", "{{ TimeDimension('metric_time', 'day') }} <= '2019-12-19'"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── WHERE filter combined with cross-model join ─────────────────
    TestCase {
        name: "where_combined_time_and_dimension",
        spec: r#"{"metrics": ["bookings"], "group_by": ["Dimension('listing__country_latest')"], "where": ["{{ TimeDimension('metric_time', 'day') }} >= '2019-12-19'", "{{ Dimension('listing__country_latest') }} = 'us'"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Saved query execution ───────────────────────────────────────
    TestCase {
        name: "saved_query_p0_booking",
        spec: r#"{"metrics": ["bookings", "instant_bookings"], "group_by": ["TimeDimension('metric_time', 'day')", "Dimension('listing__capacity_latest')"], "where": ["{{ Dimension('listing__capacity_latest') }} > 3"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Multiple dimension joins from same model ──────────────────────
    TestCase {
        name: "bookings_by_listing_country_and_capacity",
        spec: r#"{"metrics": ["bookings"], "group_by": ["Dimension('listing__country_latest')", "Dimension('listing__capacity_latest')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "revenue_by_user_state_and_day",
        spec: r#"{"metrics": ["revenue"], "group_by": ["Dimension('user__home_state_latest')", "TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Order-by descending + multi-column ──────────────────────────
    TestCase {
        name: "bookings_order_by_desc",
        spec: r#"{"metrics": ["bookings"], "group_by": ["TimeDimension('metric_time', 'day')"], "order_by": ["-bookings"], "limit": 5}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "bookings_order_by_multi_column",
        spec: r#"{"metrics": ["bookings", "booking_value"], "group_by": ["TimeDimension('metric_time', 'day')"], "order_by": ["-bookings", "+metric_time"], "limit": 10}"#,
        min_rows: 1,
        expected_value: None,
    },
    // NOTE: conversion metrics with group-by are not yet supported
    // (the compiler emits a scalar subquery regardless of group_by).
    // ── Cumulative with non-day granularity ─────────────────────────
    TestCase {
        name: "cumulative_revenue_all_time_by_month",
        spec: r#"{"metrics": ["revenue_all_time"], "group_by": ["TimeDimension('metric_time', 'month')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Metric filter + group-by combination ────────────────────────
    TestCase {
        name: "metric_filter_instant_by_day",
        spec: r#"{"metrics": ["instant_booking_value"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Quarter granularity ─────────────────────────────────────────
    TestCase {
        name: "bookings_by_quarter",
        spec: r#"{"metrics": ["bookings"], "group_by": ["TimeDimension('metric_time', 'quarter')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Double-underscore granularity syntax (JSON notation) ────────
    TestCase {
        name: "bookings_double_underscore_day",
        spec: r#"{"metrics": ["bookings"], "group_by": ["metric_time__day"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Fill nulls with spine + where filter ────────────────────────
    TestCase {
        name: "fill_nulls_spine_with_where",
        spec: r#"{"metrics": ["bookings_fill_nulls_with_spine"], "group_by": ["TimeDimension('metric_time', 'day')"], "where": ["{{ TimeDimension('metric_time', 'day') }} >= '2019-12-18'"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Cumulative + where filter ───────────────────────────────────
    TestCase {
        name: "cumulative_revenue_with_where",
        spec: r#"{"metrics": ["revenue_all_time"], "group_by": ["TimeDimension('metric_time', 'day')"], "where": ["{{ TimeDimension('metric_time', 'day') }} >= '2019-12-19'"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Ratio metric + group-by dimension ───────────────────────────
    TestCase {
        name: "ratio_bookings_per_booker_by_listing_country",
        spec: r#"{"metrics": ["bookings_per_booker"], "group_by": ["Dimension('listing__country_latest')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Derived metric + where filter ───────────────────────────────
    TestCase {
        name: "derived_booking_fees_with_where",
        spec: r#"{"metrics": ["booking_fees"], "group_by": ["TimeDimension('metric_time', 'day')"], "where": ["{{ Dimension('booking__is_instant') }}"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ── Multi-model join (PK→FK direction) ─────────────────────────
    // listings is the primary model (listing = primary entity), bookings
    // is the target (listing = foreign entity). Before the bidirectional
    // edge fix this produced a CROSS JOIN.
    TestCase {
        name: "listings_and_bookings_pk_fk_join",
        spec: r#"{"metrics": ["listings", "bookings"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "listings_and_bookings_by_listing",
        spec: r#"{"metrics": ["listings", "bookings"], "group_by": ["Entity('listing')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Multi-metric: constrained + non-constrained (same model)
    // ═══════════════════════════════════════════════════════════════════
    TestCase {
        name: "constrained_with_non_constrained_same_src",
        spec: r#"{"metrics": ["booking_value", "instant_booking_value"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "constrained_with_non_constrained_diff_src",
        spec: r#"{"metrics": ["instant_booking_value", "views"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "ratio_with_non_ratio",
        spec: r#"{"metrics": ["bookings", "bookings_per_view"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "query_with_3_metrics",
        spec: r#"{"metrics": ["bookings", "bookings_per_booker", "booking_value"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "metrics_with_different_agg_time_dims",
        spec: r#"{"metrics": ["booking_value", "booking_payments"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "shared_alias_derived_same",
        spec: r#"{"metrics": ["derived_shared_alias_1a", "derived_shared_alias_1b"], "group_by": ["Dimension('booking__is_instant')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "shared_alias_derived_different",
        spec: r#"{"metrics": ["derived_shared_alias_1a", "derived_shared_alias_2"], "group_by": ["Dimension('booking__is_instant')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "two_metrics_null_dim_values",
        spec: r#"{"metrics": ["bookings", "views"], "group_by": ["TimeDimension('metric_time', 'day')", "Dimension('listing__is_lux_latest')", "Dimension('listing__country_latest')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "three_metrics_null_dim_values",
        spec: r#"{"metrics": ["bookings", "views", "listings"], "group_by": ["Dimension('listing__is_lux_latest')", "Dimension('listing__country_latest')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "multi_metrics_no_dims",
        spec: r#"{"metrics": ["bookings", "listings"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "multi_metrics_ordered_by_metric",
        spec: r#"{"metrics": ["bookings", "booking_value"], "group_by": ["TimeDimension('metric_time', 'day')"], "order_by": ["-booking_value"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "multi_metrics_ordered_by_dimension",
        spec: r#"{"metrics": ["bookings", "booking_value"], "group_by": ["TimeDimension('metric_time', 'day')"], "order_by": ["-metric_time"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "multiple_cumulative_metrics",
        spec: r#"{"metrics": ["revenue_all_time", "trailing_2_months_revenue"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Ported from itest_metrics.yaml — single-metric tests
    // ═══════════════════════════════════════════════════════════════════
    TestCase {
        name: "constrained_metric_with_where",
        spec: r#"{"metrics": ["instant_booking_value"], "group_by": ["TimeDimension('metric_time', 'day')"], "where": ["{{ Dimension('booking__is_instant') }}"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "constrained_metric_with_user_constraint",
        spec: r#"{"metrics": ["instant_booking_value"], "group_by": ["TimeDimension('metric_time', 'day')"], "where": ["{{ Dimension('listing__is_lux_latest') }}"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "count_distinct_by_is_instant",
        spec: r#"{"metrics": ["bookers"], "group_by": ["TimeDimension('metric_time', 'day')", "Dimension('booking__is_instant')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "count_distinct_with_constraint",
        spec: r#"{"metrics": ["bookers"], "group_by": ["TimeDimension('metric_time', 'day')"], "where": ["{{ Dimension('booking__is_instant') }}"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "ratio_sort_by_metric_time",
        spec: r#"{"metrics": ["bookings_per_booker"], "group_by": ["TimeDimension('metric_time', 'day')"], "order_by": ["+metric_time"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "booking_payments_by_metric_time",
        spec: r#"{"metrics": ["booking_payments"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "derived_non_referred_bookings_pct_by_day",
        spec: r#"{"metrics": ["non_referred_bookings_pct"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "offset_to_grain_by_day",
        spec: r#"{"metrics": ["bookings_growth_since_start_of_month"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "derived_booking_value_sub_instant_by_day",
        spec: r#"{"metrics": ["booking_value_sub_instant"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "nested_derived_booking_value_sub_instant_add_10_by_day",
        spec: r#"{"metrics": ["booking_value_sub_instant_add_10"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "offset_to_grain_by_week",
        spec: r#"{"metrics": ["bookings_growth_since_start_of_month"], "group_by": ["TimeDimension('metric_time', 'week')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "offset_to_grain_by_month_and_week",
        spec: r#"{"metrics": ["bookings_growth_since_start_of_month"], "group_by": ["TimeDimension('metric_time', 'month')", "TimeDimension('metric_time', 'week')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "offset_to_grain_with_agg_time_dim",
        spec: r#"{"metrics": ["bookings_growth_since_start_of_month"], "group_by": ["TimeDimension('booking__ds', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "join_to_time_spine_by_metric_time",
        spec: r#"{"metrics": ["bookings_join_to_time_spine"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "join_to_time_spine_with_where_filter",
        spec: r#"{"metrics": ["bookings_join_to_time_spine"], "group_by": ["TimeDimension('metric_time', 'day')"], "where": ["{{ Dimension('booking__is_instant') }}"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Ported from itest_metrics.yaml — tests using NEW metric definitions
    // ═══════════════════════════════════════════════════════════════════
    TestCase {
        name: "identifier_constrained_metric",
        spec: r#"{"metrics": ["booking_value_for_non_null_listing_id"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "lux_listings_filtered",
        spec: r#"{"metrics": ["lux_listings"], "group_by": ["Dimension('listing__country_latest')"], "where": ["{{ Dimension('listing__country_latest') }} = 'us'"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "derived_metrics_with_null_dimension_values",
        spec: r#"{"metrics": ["booking_value_per_view"], "group_by": ["Dimension('listing__is_lux_latest')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "derived_offset_window_2_weeks",
        spec: r#"{"metrics": ["bookings_growth_2_weeks"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "derived_offset_5_day_lag",
        spec: r#"{"metrics": ["bookings_5_day_lag"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "derived_offset_5_day_lag_by_month",
        spec: r#"{"metrics": ["bookings_5_day_lag"], "group_by": ["TimeDimension('metric_time', 'month')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "derived_offset_5_day_lag_by_month_and_week",
        spec: r#"{"metrics": ["bookings_5_day_lag"], "group_by": ["TimeDimension('metric_time', 'month')", "TimeDimension('metric_time', 'week')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "derived_offset_window_and_offset_to_grain",
        spec: r#"{"metrics": ["bookings_month_start_compared_to_1_month_prior"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "derived_offset_window_and_offset_to_grain_by_year",
        spec: r#"{"metrics": ["bookings_month_start_compared_to_1_month_prior"], "group_by": ["TimeDimension('metric_time', 'year')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "nested_derived_instant_plus_non_referred",
        spec: r#"{"metrics": ["instant_plus_non_referred_bookings_pct"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "derived_bookings_per_lux_listing",
        spec: r#"{"metrics": ["bookings_per_lux_listing_derived"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "derived_offset_cumulative_metric",
        spec: r#"{"metrics": ["every_2_days_bookers_2_days_ago"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "offset_window_with_agg_time_dim",
        spec: r#"{"metrics": ["bookings_growth_2_weeks"], "group_by": ["TimeDimension('booking__ds', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "fill_nulls_with_0_by_metric_time",
        spec: r#"{"metrics": ["bookings_fill_nulls_with_0"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "fill_nulls_with_0_by_month",
        spec: r#"{"metrics": ["bookings_fill_nulls_with_0"], "group_by": ["TimeDimension('metric_time', 'month')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "fill_nulls_without_time_spine_by_day",
        spec: r#"{"metrics": ["bookings_fill_nulls_with_0_without_time_spine"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "cumulative_fill_nulls_with_0",
        spec: r#"{"metrics": ["every_two_days_bookers_fill_nulls_with_0"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "derived_fill_nulls_for_one_input_metric",
        spec: r#"{"metrics": ["bookings_growth_2_weeks_fill_nulls_with_0_for_non_offset"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "nested_derived_outer_offset",
        spec: r#"{"metrics": ["bookings_offset_twice"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "nested_derived_offset_multiple_inputs",
        spec: r#"{"metrics": ["booking_fees_since_start_of_month"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "metric_with_metric_in_where_filter",
        spec: r#"{"metrics": ["active_listings"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Metric-in-where-filter: query-level {{ Metric() }} in WHERE clause
    // ═══════════════════════════════════════════════════════════════════
    TestCase {
        name: "query_metric_in_where_filter",
        spec: r#"{"metrics": ["listings"], "where": ["{{ Metric('bookings', ['listing']) }} > 2"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "cumulative_metric_in_where_filter",
        spec: r#"{"metrics": ["listings"], "where": ["{{ Metric('revenue_all_time', ['user']) }} > 2"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "multiple_metrics_in_where_filter",
        spec: r#"{"metrics": ["listings"], "where": ["{{ Metric('bookings', ['listing']) }} > 2 AND {{ Metric('bookers', ['listing']) }} > 1"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Fill nulls: additional variations
    // ═══════════════════════════════════════════════════════════════════
    TestCase {
        name: "fill_nulls_with_0_multi_metric",
        spec: r#"{"metrics": ["bookings_fill_nulls_with_0", "views"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "fill_nulls_derived_multi_metric",
        spec: r#"{"metrics": ["bookings_growth_2_weeks_fill_nulls_with_0_for_non_offset", "booking_value"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "fill_nulls_multi_metric_with_categorical_dim",
        spec: r#"{"metrics": ["bookings_fill_nulls_with_0_without_time_spine", "views"], "group_by": ["TimeDimension('metric_time', 'day')", "Dimension('listing__is_lux_latest')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Nested offset with constraints and agg_time_dim
    // ═══════════════════════════════════════════════════════════════════
    TestCase {
        name: "nested_offset_with_agg_time_dim",
        spec: r#"{"metrics": ["bookings_offset_twice"], "group_by": ["TimeDimension('booking__ds', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "ratio_with_metric_filter_on_metric",
        spec: r#"{"metrics": ["popular_listing_bookings_per_booker"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Join to time spine with filters
    // ═══════════════════════════════════════════════════════════════════
    TestCase {
        name: "join_to_time_spine_with_queried_filter_and_dim",
        spec: r#"{"metrics": ["bookings_join_to_time_spine"], "group_by": ["TimeDimension('metric_time', 'day')", "Dimension('booking__is_instant')"], "where": ["{{ Dimension('booking__is_instant') }}"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Metric filter with entity prefix
    // ═══════════════════════════════════════════════════════════════════
    TestCase {
        name: "metric_filter_with_entity_prefix",
        spec: r#"{"metrics": ["listings"], "where": ["{{ Metric('views', ['view__listing']) }} > 2"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Date part: EXTRACT(part FROM ...) instead of DATE_TRUNC
    // ═══════════════════════════════════════════════════════════════════
    TestCase {
        name: "date_part_year",
        spec: r#"{"metrics": ["bookings"], "group_by": ["TimeDimension('metric_time', 'day', date_part='year')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "date_part_year_non_metric_time",
        spec: r#"{"metrics": ["bookings"], "group_by": ["TimeDimension('booking__ds', 'day', date_part='year')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "date_part_multiple",
        spec: r#"{"metrics": ["bookings"], "group_by": ["TimeDimension('metric_time', 'day', date_part='quarter')", "TimeDimension('metric_time', 'day', date_part='dow')", "TimeDimension('metric_time', 'day', date_part='doy')", "TimeDimension('metric_time', 'day', date_part='day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "date_part_derived_offset_month",
        spec: r#"{"metrics": ["bookings_5_day_lag"], "group_by": ["TimeDimension('metric_time', 'day', date_part='month')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "date_part_count_distinct_dow",
        spec: r#"{"metrics": ["bookers"], "group_by": ["TimeDimension('metric_time', 'day', date_part='dow')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Time constraint: WHERE metric_time BETWEEN start AND end
    // ═══════════════════════════════════════════════════════════════════
    TestCase {
        name: "time_constraint_nested_offset",
        spec: r#"{"metrics": ["bookings_offset_twice"], "group_by": ["TimeDimension('metric_time', 'day')"], "time_constraint": ["2019-12-05", "2019-12-10"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "time_constraint_offset_window",
        spec: r#"{"metrics": ["bookings_5_day_lag"], "group_by": ["TimeDimension('metric_time', 'day')"], "time_constraint": ["2019-12-05", "2019-12-10"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "time_constraint_cumulative_offset",
        spec: r#"{"metrics": ["every_2_days_bookers_2_days_ago"], "group_by": ["TimeDimension('metric_time', 'day')"], "time_constraint": ["2019-12-01", "2019-12-10"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "time_constraint_join_to_time_spine",
        spec: r#"{"metrics": ["bookings_join_to_time_spine"], "group_by": ["TimeDimension('metric_time', 'day')"], "time_constraint": ["2019-12-19", "2019-12-19"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Remaining STANDARD tests from itest_metrics.yaml
    // ═══════════════════════════════════════════════════════════════════
    TestCase {
        name: "lux_listings_by_country_filtered",
        spec: r#"{"metrics": ["lux_listings"], "group_by": ["Dimension('listing__country_latest')"], "where": ["{{ Dimension('listing__country_latest') }} = 'us'"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "same_measure_constrained_and_unconstrained",
        spec: r#"{"metrics": ["booking_value", "instant_booking_value"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "fill_nulls_with_0_non_metric_time",
        spec: r#"{"metrics": ["bookings_fill_nulls_with_0"], "group_by": ["TimeDimension('booking__paid_at', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "fill_nulls_with_0_agg_time_dim",
        spec: r#"{"metrics": ["bookings_fill_nulls_with_0"], "group_by": ["TimeDimension('booking__ds', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "fill_nulls_with_0_categorical_dim",
        spec: r#"{"metrics": ["bookings_fill_nulls_with_0"], "group_by": ["Dimension('booking__is_instant')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "ratio_with_zero_denominator",
        spec: r#"{"metrics": ["bookings_per_dollar"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Remaining DATE_PART tests
    // ═══════════════════════════════════════════════════════════════════
    TestCase {
        name: "date_part_offset_agg_time_dim_doy",
        spec: r#"{"metrics": ["bookings_5_day_lag"], "group_by": ["TimeDimension('booking__ds', 'day', date_part='doy')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Remaining TIME_CONSTRAINT tests
    // ═══════════════════════════════════════════════════════════════════
    TestCase {
        name: "time_constraint_join_to_time_spine_queried",
        spec: r#"{"metrics": ["bookings_join_to_time_spine"], "group_by": ["TimeDimension('metric_time', 'day')"], "time_constraint": ["2019-12-19", "2019-12-19"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Subdaily tests: archived_users with hour-level agg_time_dimension
    // ═══════════════════════════════════════════════════════════════════
    TestCase {
        name: "simple_subdaily_metric_default_day",
        spec: r#"{"metrics": ["archived_users"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: Some(7.0),
    },
    TestCase {
        name: "simple_subdaily_metric_default_hour",
        spec: r#"{"metrics": ["archived_users"], "group_by": ["TimeDimension('metric_time', 'hour')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "subdaily_offset_window",
        spec: r#"{"metrics": ["subdaily_offset_window_metric"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "subdaily_offset_grain_to_date",
        spec: r#"{"metrics": ["subdaily_offset_grain_to_date_metric"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "subdaily_join_to_time_spine",
        spec: r#"{"metrics": ["subdaily_join_to_time_spine_metric"], "group_by": ["TimeDimension('metric_time', 'day')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "subdaily_metric_by_dimension",
        spec: r#"{"metrics": ["archived_users"], "group_by": ["TimeDimension('metric_time', 'day')", "Dimension('user__home_state')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "subdaily_metric_hour_by_dimension",
        spec: r#"{"metrics": ["archived_users"], "group_by": ["TimeDimension('metric_time', 'hour')", "Dimension('user__home_state')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Derived/ratio metric in WHERE filter
    // ═══════════════════════════════════════════════════════════════════
    TestCase {
        name: "derived_metric_in_where_filter",
        spec: r#"{"metrics": ["bookings"], "where": ["{{ Metric('views_times_booking_value', ['listing']) }} > 1000"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "ratio_metric_in_where_filter",
        spec: r#"{"metrics": ["bookings"], "where": ["{{ Metric('bookings_per_view', ['listing']) }} > 0.5"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // WHERE-pushdown-on-offset
    // ═══════════════════════════════════════════════════════════════════
    TestCase {
        name: "offset_with_where_on_grouped_dim",
        spec: r#"{"metrics": ["bookings_5_day_lag"], "group_by": ["TimeDimension('metric_time', 'day')", "Dimension('booking__is_instant')"], "where": ["NOT {{ Dimension('booking__is_instant') }}"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "nested_offset_with_where_constraint",
        spec: r#"{"metrics": ["bookings_growth_2_weeks"], "group_by": ["TimeDimension('metric_time', 'day')", "Dimension('booking__is_instant')"], "where": ["{{ Dimension('booking__is_instant') }}"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // Third-hop join tests
    // ═══════════════════════════════════════════════════════════════════
    TestCase {
        name: "third_hop_bookings_by_region",
        spec: r#"{"metrics": ["bookings"], "group_by": ["Dimension('region__region_name')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "third_hop_revenue_by_region",
        spec: r#"{"metrics": ["revenue"], "group_by": ["Dimension('region__region_name')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // 3+ input derived metric: COALESCE join keys
    // ═══════════════════════════════════════════════════════════════════
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
    TestCase {
        name: "three_input_derived_no_dims",
        spec: r#"{"metrics": ["bookings_plus_views_plus_listings"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // NULL-safe join: IS NOT DISTINCT FROM
    // ═══════════════════════════════════════════════════════════════════
    TestCase {
        name: "null_safe_join_multi_metric_nullable_dim",
        spec: r#"{"metrics": ["bookings", "views"], "group_by": ["Dimension('listing__is_lux_latest')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    TestCase {
        name: "null_safe_join_ratio_nullable_dim",
        spec: r#"{"metrics": ["bookings_per_view"], "group_by": ["Dimension('listing__is_lux_latest')"]}"#,
        min_rows: 1,
        expected_value: None,
    },
    // ═══════════════════════════════════════════════════════════════════
    // fill_nulls_with in derived expression
    // ═══════════════════════════════════════════════════════════════════
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
// Shared scorecard runner
// ═══════════════════════════════════════════════════════════════════════════

pub fn setup_metric_store(database: &str, schema: &str) -> InMemoryMetricStore {
    let manifest = build_semantic_manifest(database, schema);
    InMemoryMetricStore::from_manifest(&manifest)
}

#[allow(clippy::type_complexity)]
pub fn run_scorecard(
    store: &mut InMemoryMetricStore,
    dialect: Dialect,
    dialect_name: &str,
    execute: &mut dyn FnMut(&str, &str) -> Result<Option<Vec<RecordBatch>>, String>,
) {
    let mut pass = 0u32;
    let mut fail = 0u32;
    let mut skip = 0u32;
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
        let sql = match compile(store, &spec, dialect) {
            Ok(s) => s,
            Err(e) => {
                fail += 1;
                results.push((tc.name.to_string(), false, format!("compile error: {e}")));
                continue;
            }
        };

        match execute(tc.name, &sql) {
            Ok(Some(batches)) => {
                let rows: usize = batches.iter().map(|b| b.num_rows()).sum();
                if rows < tc.min_rows {
                    fail += 1;
                    results.push((
                        tc.name.to_string(),
                        false,
                        format!("expected >= {} rows, got {rows}\n  SQL: {sql}", tc.min_rows),
                    ));
                } else if let Some(expected) = tc.expected_value {
                    let scalar = extract_scalar(&batches);
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
                            results.push((tc.name.to_string(), false, format!("value mismatch: expected {expected}, got {actual}\n  SQL: {sql}")));
                        }
                    } else {
                        fail += 1;
                        results.push((tc.name.to_string(), false, format!("expected value {expected} but could not extract scalar\n  SQL: {sql}")));
                    }
                } else {
                    pass += 1;
                    results.push((tc.name.to_string(), true, format!("{rows} rows")));
                }
            }
            Ok(None) => {
                skip += 1;
            }
            Err(msg) => {
                fail += 1;
                results.push((tc.name.to_string(), false, msg));
            }
        }
    }

    let bar = "=".repeat(70);
    eprintln!("\n{bar}");
    eprintln!("{dialect_name} Compatibility Scorecard");
    eprintln!("{bar}");
    for (name, ok, detail) in &results {
        let icon = if *ok { "PASS" } else { "FAIL" };
        eprintln!("  [{icon}] {name}: {detail}");
    }
    eprintln!("{bar}");
    let total = pass + fail;
    eprintln!(
        "  Total: {total}  Pass: {pass}  Fail: {fail}  Skip: {skip}  ({:.0}%)",
        if total > 0 {
            (pass as f64 / total as f64) * 100.0
        } else {
            0.0
        }
    );
    eprintln!("{bar}\n");

    assert_eq!(
        fail, 0,
        "{fail} of {total} {dialect_name} compatibility tests failed"
    );
}

pub fn extract_scalar(batches: &[RecordBatch]) -> Option<f64> {
    let b = batches.first()?;
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
}

// ═══════════════════════════════════════════════════════════════════════════
// Ported test infrastructure — shared between snowflake_compat and others
// ═══════════════════════════════════════════════════════════════════════════

/// Rewrite all relation_names in a manifest to use a specific database/schema.
pub fn rewrite_manifest_relations(manifest: &mut serde_json::Value, database: &str, schema: &str) {
    if let Some(models) = manifest
        .get_mut("semantic_models")
        .and_then(|v| v.as_array_mut())
    {
        for model in models {
            if let Some(nr) = model.get_mut("node_relation") {
                let alias = nr
                    .get("alias")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_uppercase();
                nr["alias"] = json!(alias);
                nr["database"] = json!(database);
                nr["schema_name"] = json!(schema);
                nr["relation_name"] = json!(format!("\"{database}\".\"{schema}\".\"{alias}\""));
            }
        }
    }
    if let Some(pc) = manifest.get_mut("project_configuration") {
        if let Some(spines) = pc.get_mut("time_spines").and_then(|v| v.as_array_mut()) {
            for spine in spines {
                if let Some(nr) = spine.get_mut("node_relation") {
                    let alias = nr
                        .get("alias")
                        .and_then(|v| v.as_str())
                        .unwrap_or("mf_time_spine")
                        .to_uppercase();
                    nr["alias"] = json!(alias);
                    nr["database"] = json!(database);
                    nr["schema_name"] = json!(schema);
                    nr["relation_name"] = json!(format!("\"{database}\".\"{schema}\".\"{alias}\""));
                }
            }
        }
    }
}

/// Transform DuckDB-flavored SQL DDL to Snowflake by schema-qualifying table names.
pub fn schema_qualify_sql(sql: &str, schema: &str) -> Vec<String> {
    let mut stmts = Vec::new();
    for raw_stmt in sql.split(';') {
        // Strip leading comment lines before checking if the statement is empty.
        let stripped: String = raw_stmt
            .lines()
            .filter(|line| !line.trim_start().starts_with("--"))
            .collect::<Vec<_>>()
            .join("\n");
        let stmt = stripped.trim();
        if stmt.is_empty() {
            continue;
        }
        let qualified = stmt
            .replace("CREATE TABLE ", &format!("CREATE TABLE {schema}."))
            .replace("INSERT INTO ", &format!("INSERT INTO {schema}."));
        stmts.push(qualified);
    }
    stmts
}

/// Generate Snowflake DDL for all ported test data tables.
pub fn snowflake_ported_data_ddl(schema: &str) -> Vec<String> {
    let sql_files: &[&str] = &[
        include_str!("../data/tables_simple_model_extra.sql"),
        include_str!("../data/tables_time_spine.sql"),
        include_str!("../data/tables_extended_date_model.sql"),
        include_str!("../data/tables_multi_hop_model.sql"),
        include_str!("../data/tables_scd_model.sql"),
    ];
    let mut stmts = Vec::new();
    for file_sql in sql_files {
        stmts.extend(schema_qualify_sql(file_sql, schema));
    }
    // fct_bookings_dt: copy of fct_bookings with dt instead of ds
    stmts.push(format!(
        "CREATE TABLE {schema}.fct_bookings_dt AS \
         SELECT ds AS dt, listing_id, guest_id, host_id, is_instant, booking_value, referrer_id \
         FROM {schema}.fct_bookings"
    ));
    stmts
}

/// Ported test case loaded from JSON.
#[derive(serde::Deserialize)]
pub struct PortedTestCase {
    pub name: String,
    #[allow(dead_code)]
    pub file: String,
    pub model: String,
    pub spec: PortedSpec,
    pub check_query: String,
    pub check_order: bool,
    #[allow(dead_code)]
    pub allow_empty: bool,
    pub skip_reason: Option<String>,
    pub min_max_only: bool,
}

#[derive(serde::Deserialize)]
pub struct PortedSpec {
    #[serde(default)]
    pub metrics: Vec<String>,
    #[serde(default)]
    pub group_by: Vec<String>,
    #[serde(default, rename = "where")]
    pub where_filters: Vec<String>,
    #[serde(default)]
    pub order_by: Vec<String>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub time_constraint: Option<Vec<String>>,
    #[serde(default = "default_true")]
    pub apply_group_by: bool,
}

fn default_true() -> bool {
    true
}

impl PortedSpec {
    pub fn to_json(&self) -> String {
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

pub fn load_ported_test_cases() -> Vec<PortedTestCase> {
    serde_json::from_str(include_str!("../data/ported_test_cases.json"))
        .expect("failed to parse ported_test_cases.json")
}

/// Map of model_name -> InMemoryMetricStore for ported tests.
pub type PortedStores = HashMap<String, InMemoryMetricStore>;

/// Run the ported test scorecard against a given dialect.
///
/// `execute` is called with (test_name, compiled_sql) and should return
/// the result batches, or None to skip.
#[allow(clippy::type_complexity)]
pub fn run_ported_scorecard(
    stores: &mut PortedStores,
    dialect: Dialect,
    dialect_name: &str,
    execute: &mut dyn FnMut(&str, &str) -> Result<Option<Vec<RecordBatch>>, String>,
) {
    let test_cases = load_ported_test_cases();

    let mut pass = 0u32;
    let mut fail = 0u32;
    let mut skip = 0u32;
    let mut results: Vec<(String, bool, String)> = Vec::new();

    for tc in &test_cases {
        let store = match stores.get_mut(tc.model.as_str()) {
            Some(s) => s,
            None => {
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
                results.push((tc.name.clone(), false, format!("parse error: {e}")));
                continue;
            }
        };
        let sql = match compile(store, &spec, dialect) {
            Ok(s) => s,
            Err(e) => {
                fail += 1;
                results.push((tc.name.clone(), false, format!("compile error: {e}")));
                continue;
            }
        };

        match execute(&tc.name, &sql) {
            Ok(Some(batches)) => {
                let rows: usize = batches.iter().map(|b| b.num_rows()).sum();
                pass += 1;
                results.push((tc.name.clone(), true, format!("{rows} rows")));
            }
            Ok(None) => {
                skip += 1;
            }
            Err(msg) => {
                fail += 1;
                results.push((tc.name.clone(), false, msg));
            }
        }
    }

    let bar = "=".repeat(70);
    eprintln!("\n{bar}");
    eprintln!("{dialect_name} Ported Tests Scorecard");
    eprintln!("{bar}");
    for (name, ok, detail) in &results {
        let icon = if *ok { "PASS" } else { "FAIL" };
        eprintln!("  [{icon}] {name}: {detail}");
    }
    eprintln!("{bar}");
    let total = pass + fail;
    eprintln!(
        "  Total: {total}  Pass: {pass}  Fail: {fail}  Skip: {skip}  ({:.0}%)",
        if total > 0 {
            (pass as f64 / total as f64) * 100.0
        } else {
            0.0
        }
    );
    eprintln!("{bar}\n");

    assert_eq!(
        fail, 0,
        "{fail} of {total} {dialect_name} ported tests failed"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Result comparison utilities (shared between DuckDB and Snowflake tests)
// ═══════════════════════════════════════════════════════════════════════════

pub fn extract_named_rows(batches: &[RecordBatch]) -> (Vec<String>, Vec<Vec<String>>) {
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

pub fn compare_results(
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

    let col_mapping: Vec<Option<usize>> = ref_names
        .iter()
        .map(|ref_name| {
            if let Some(idx) = our_names.iter().position(|n| n == ref_name) {
                return Some(idx);
            }
            for (idx, our_name) in our_names.iter().enumerate() {
                if our_name.starts_with(ref_name) || ref_name.starts_with(our_name) {
                    return Some(idx);
                }
                if ref_name.contains(our_name) || our_name.contains(ref_name) {
                    return Some(idx);
                }
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

    let mapped_count = col_mapping.iter().filter(|m| m.is_some()).count();
    let use_name_mapping = mapped_count > 0 && mapped_count >= ref_names.len() / 2;

    let mut our_projected: Vec<Vec<String>>;
    let ref_projected: Vec<Vec<String>>;

    if use_name_mapping {
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
        our_projected = our_data;
        ref_projected = ref_data;
    }

    let mut ref_sorted = ref_projected;
    if !check_order {
        our_projected.sort();
        ref_sorted.sort();
    }

    for (i, (our_row, ref_row)) in our_projected.iter().zip(ref_sorted.iter()).enumerate() {
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
            // Normalize date/timestamp: "2019-12-01" == "2019-12-01 00:00:00"
            let ours_norm = ours.strip_suffix(" 00:00:00").unwrap_or(ours);
            let theirs_norm = theirs.strip_suffix(" 00:00:00").unwrap_or(theirs);
            if ours_norm == theirs_norm {
                continue;
            }
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

/// Rewrite a DuckDB-flavored check_query for Snowflake execution.
pub fn rewrite_check_query_for_snowflake(
    check_query: &str,
    database: &str,
    schema: &str,
) -> String {
    // Step 1: replace `main.table_name` with `DATABASE.SCHEMA.TABLE_NAME`.
    let input = check_query;
    let mut out = String::with_capacity(input.len() * 2);
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 5 <= bytes.len() && &input[i..i + 5] == "main." {
            let at_boundary =
                i == 0 || !bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b'_';
            if at_boundary {
                let start = i + 5;
                let mut end = start;
                while end < bytes.len()
                    && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
                {
                    end += 1;
                }
                if end > start {
                    let table = input[start..end].to_uppercase();
                    out.push_str(&format!("{database}.{schema}.{table}"));
                    i = end;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }

    // Step 2: DuckDB→Snowflake syntax adaptations.
    let mut result = out;
    result = result.replace("GEN_RANDOM_UUID()", "UUID_STRING()");
    result = result.replace("EXTRACT(isodow FROM", "EXTRACT(DAYOFWEEKISO FROM");
    result = result.replace("EXTRACT(doy FROM", "EXTRACT(DAYOFYEAR FROM");

    // DuckDB: `APPROX_QUANTILE(expr, p)` → Snowflake: `APPROX_PERCENTILE(expr, p)`
    result = result.replace("APPROX_QUANTILE(", "APPROX_PERCENTILE(");

    // DuckDB: `expr + INTERVAL ((dynamic_expr)) gran` → Snowflake: `DATEADD('GRAN', dynamic_expr, expr)`
    result = rewrite_dynamic_intervals(&result);

    // DuckDB: `INTERVAL N gran` → Snowflake: `INTERVAL 'N GRAN'`
    result = rewrite_duckdb_intervals(&result);
    result
}

fn rewrite_dynamic_intervals(sql: &str) -> String {
    let grans = [
        "day", "days", "month", "months", "year", "years", "hour", "hours",
    ];
    let mut result = sql.to_string();
    // Match: `<ident> + INTERVAL ((<expr>)) <gran>`
    // Replace with: `DATEADD('<GRAN>', <expr>, <ident>)`
    for gran in grans {
        loop {
            let pattern = "+ INTERVAL ((";
            if let Some(plus_pos) = result.find(pattern) {
                // Find the base expression before `+`: scan backwards for the identifier.
                let before = &result[..plus_pos].trim_end();
                // Find the start of the last identifier/expression token.
                let base_start = before
                    .rfind(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '.')
                    .map(|p| p + 1)
                    .unwrap_or(0);
                let base = before[base_start..].to_string();

                // Find the matching `))`
                let inner_start = plus_pos + pattern.len();
                if let Some(close_pos) = result[inner_start..].find("))") {
                    let expr = result[inner_start..inner_start + close_pos].to_string();
                    let after_close = &result[inner_start + close_pos + 2..].trim_start();
                    // Check if next word is the granularity.
                    let word_end = after_close
                        .find(|c: char| !c.is_ascii_alphabetic())
                        .unwrap_or(after_close.len());
                    let word = &after_close[..word_end];
                    if word.eq_ignore_ascii_case(gran) {
                        let gran_upper = gran.to_uppercase();
                        let total_end = result.len() - after_close.len()
                            + word_end
                            + (result[inner_start + close_pos + 2..].len() - after_close.len());
                        let replacement = format!("DATEADD('{gran_upper}', {expr}, {base})");
                        // Replace from base_start to total_end
                        result = format!(
                            "{}{}{}",
                            &result[..base_start],
                            replacement,
                            &result[total_end..]
                        );
                        continue;
                    }
                }
            }
            break;
        }
    }
    result
}

fn rewrite_duckdb_intervals(sql: &str) -> String {
    let grans = [
        "day", "days", "week", "weeks", "month", "months", "year", "years", "hour", "hours",
        "minute", "minutes", "second", "seconds",
    ];
    let mut out = String::with_capacity(sql.len());
    let mut chars = sql.char_indices().peekable();
    while let Some((i, _)) = chars.peek() {
        let rest = &sql[*i..];
        if rest.len() >= 10 && rest[..9].eq_ignore_ascii_case("INTERVAL ") {
            let after_kw = &rest[9..];
            // Check if already quoted: INTERVAL '...'
            if after_kw.starts_with('\'') {
                out.push_str("INTERVAL ");
                for _ in 0..9 {
                    chars.next();
                }
                continue;
            }
            // Try to match: <digits><whitespace><granularity>
            let num_end = after_kw
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(after_kw.len());
            if num_end > 0 {
                let num = &after_kw[..num_end];
                let space_start = num_end;
                let trimmed = after_kw[space_start..].trim_start();
                let space_len = after_kw[space_start..].len() - trimmed.len();
                if space_len > 0 {
                    // Check if next word is a granularity.
                    let word_end = trimmed
                        .find(|c: char| !c.is_ascii_alphabetic())
                        .unwrap_or(trimmed.len());
                    let word = &trimmed[..word_end];
                    if grans.iter().any(|g| g.eq_ignore_ascii_case(word)) {
                        let total_consumed = 9 + num_end + space_len + word_end;
                        out.push_str(&format!("INTERVAL '{} {}'", num, word.to_uppercase()));
                        for _ in 0..total_consumed {
                            chars.next();
                        }
                        continue;
                    }
                }
            }
        }
        let (_, c) = chars.next().unwrap();
        out.push(c);
    }
    out
}

/// Wrap compiled SQL with min/max projection for tests that only check
/// extreme values of grouped dimensions.
pub fn wrap_min_max_only(sql: &str, spec: &PortedSpec) -> String {
    let col_names: Vec<String> = spec
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
        sql.replace('\n', "\n  ")
    )
}

// ═══════════════════════════════════════════════════════════════════════════
// Parquet "tome" — single file housing all recordings for one dialect
// ═══════════════════════════════════════════════════════════════════════════

fn batches_to_ipc(batches: &[RecordBatch]) -> Vec<u8> {
    let schema = batches
        .first()
        .map(|b| (*b.schema()).clone())
        .unwrap_or_else(Schema::empty);
    let opts = IpcWriteOptions::default()
        .try_with_compression(Some(arrow_ipc::CompressionType::LZ4_FRAME))
        .expect("lz4 compression");
    let mut buf = Vec::new();
    let mut w = StreamWriter::try_new_with_options(&mut buf, &schema, opts).expect("ipc writer");
    for b in batches {
        w.write(b).expect("ipc write");
    }
    w.finish().expect("ipc finish");
    drop(w);
    buf
}

fn ipc_to_batches(data: &[u8]) -> Vec<RecordBatch> {
    let reader = StreamReader::try_new(Cursor::new(data), None).expect("ipc reader");
    reader
        .into_iter()
        .collect::<Result<Vec<_>, ArrowError>>()
        .expect("ipc read batches")
}

/// A tome writer collects recordings then flushes them to a single parquet file.
pub struct TomeWriter {
    entries: Vec<(String, Vec<u8>)>,
}

impl TomeWriter {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn insert(&mut self, name: &str, batches: &[RecordBatch]) {
        if batches.is_empty() {
            return;
        }
        self.entries
            .push((name.to_string(), batches_to_ipc(batches)));
    }

    pub fn write(self, path: &Path) {
        use arrow_array::{BinaryArray, StringArray};

        let names: Vec<&str> = self.entries.iter().map(|(n, _)| n.as_str()).collect();
        let blobs: Vec<&[u8]> = self.entries.iter().map(|(_, d)| d.as_slice()).collect();
        let batch = RecordBatch::try_from_iter(vec![
            ("name", Arc::new(StringArray::from(names)) as _),
            ("ipc_data", Arc::new(BinaryArray::from(blobs)) as _),
        ])
        .expect("tome batch");

        let file = std::fs::File::create(path).expect("create tome");
        let props = parquet::file::properties::WriterProperties::builder()
            .set_compression(parquet::basic::Compression::ZSTD(
                parquet::basic::ZstdLevel::try_new(3).unwrap(),
            ))
            .build();
        let mut writer = parquet::arrow::ArrowWriter::try_new(file, batch.schema(), Some(props))
            .expect("parquet writer");
        writer.write(&batch).expect("parquet write");
        writer.close().expect("parquet close");
    }
}

/// Migrate a directory of `.arrow` IPC files into a single parquet tome.
pub fn migrate_arrow_dir_to_tome(arrow_dir: &Path, tome_path: &Path) {
    let mut tome = TomeWriter::new();
    let mut entries: Vec<_> = std::fs::read_dir(arrow_dir)
        .expect("read arrow dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "arrow"))
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = path.file_stem().unwrap().to_str().unwrap().to_string();
        let data = std::fs::read(&path).expect("read arrow file");
        let batches = ipc_to_batches(&data);
        if !batches.is_empty() {
            tome.entries.push((name, batches_to_ipc(&batches)));
        }
    }
    tome.write(tome_path);
}

/// Read all recordings from a parquet tome into a map.
pub fn read_tome(path: &Path) -> HashMap<String, Vec<RecordBatch>> {
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

    let file = std::fs::File::open(path).expect("open tome");
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .expect("parquet reader builder")
        .build()
        .expect("parquet reader");

    let mut map = HashMap::new();
    for batch in reader {
        let batch = batch.expect("read parquet batch");
        let names = batch
            .column(0)
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .expect("name col");
        let blobs = batch
            .column(1)
            .as_any()
            .downcast_ref::<arrow_array::BinaryArray>()
            .expect("ipc_data col");
        for i in 0..batch.num_rows() {
            let name = names.value(i).to_string();
            let ipc = blobs.value(i);
            map.insert(name, ipc_to_batches(ipc));
        }
    }
    map
}
