# Authoring Metrics

This document shows how to author metrics for the current `dbt-metricflow`
compiler using the semantic manifest JSON shape it already consumes.

All examples here use the DuckDB-backed manifest at
[`examples/duckdb_orders_manifest.json`](examples/duckdb_orders_manifest.json).

For the CLI query surface and end-user query examples, see
[`metric-semantics.md`](metric-semantics.md).

## Start With A Semantic Model

A metric needs a semantic model to define:

- the physical relation
- the entities used for joins
- the dimensions used for slicing and filtering
- the default aggregation time dimension

Minimal example:

```json
{
  "name": "orders",
  "node_relation": {
    "alias": "orders",
    "schema_name": "main",
    "database": null,
    "relation_name": "\"main\".\"orders\""
  },
  "defaults": { "agg_time_dimension": "ordered_at" },
  "primary_entity": "order",
  "entities": [
    { "name": "order", "type": "primary", "expr": "order_id" },
    { "name": "customer", "type": "foreign", "expr": "customer_id" }
  ],
  "dimensions": [
    {
      "name": "ordered_at",
      "type": "time",
      "expr": "ordered_at",
      "type_params": { "time_granularity": "day" }
    },
    { "name": "status", "type": "categorical", "expr": "status" }
  ]
}
```

## Simple Metrics

Simple metrics are the base case: aggregate one expression from one semantic
model.

`order_count`:

```json
{
  "name": "order_count",
  "type": "simple",
  "type_params": {
    "metric_aggregation_params": {
      "semantic_model": "orders",
      "agg": "sum",
      "expr": "1",
      "agg_time_dimension": "ordered_at"
    }
  }
}
```

`revenue`:

```json
{
  "name": "revenue",
  "type": "simple",
  "type_params": {
    "metric_aggregation_params": {
      "semantic_model": "orders",
      "agg": "sum",
      "expr": "amount",
      "agg_time_dimension": "ordered_at"
    }
  }
}
```

Use a simple metric when the metric is "one aggregation over one base grain."

## Metric-Level Filters

If a filter is part of the metric's meaning, put it on the metric itself.

`completed_order_count`:

```json
{
  "name": "completed_order_count",
  "type": "simple",
  "filter": {
    "where_filters": [
      {
        "where_sql_template": "{{ Dimension('status') }} = 'completed'"
      }
    ]
  },
  "type_params": {
    "metric_aggregation_params": {
      "semantic_model": "orders",
      "agg": "sum",
      "expr": "1",
      "agg_time_dimension": "ordered_at"
    }
  }
}
```

Use a metric-level filter when the filtered version deserves its own stable
name and meaning.

## Derived Metrics

Derived metrics combine other metrics with an expression.

`average_order_value`:

```json
{
  "name": "average_order_value",
  "type": "derived",
  "type_params": {
    "expr": "revenue / order_count",
    "metrics": [
      { "name": "revenue" },
      { "name": "order_count" }
    ]
  }
}
```

Use a derived metric when the business definition is "metric A combined with
metric B" rather than "one base-table aggregation."

## Ratio Metrics

Ratio metrics are a specialized form of derived metric where the compiler
treats the numerator and denominator explicitly.

`completion_rate`:

```json
{
  "name": "completion_rate",
  "type": "ratio",
  "type_params": {
    "numerator": {
      "name": "completed_order_count",
      "filter": null,
      "alias": null,
      "offset_window": null,
      "offset_to_grain": null
    },
    "denominator": {
      "name": "order_count",
      "filter": null,
      "alias": null,
      "offset_window": null,
      "offset_to_grain": null
    }
  }
}
```

Use a ratio metric when you want the manifest to say clearly which metric is
the numerator and which is the denominator.

## Cumulative Metrics

Cumulative metrics apply a rolling or grain-to-date window.

`trailing_2_day_revenue`:

```json
{
  "name": "trailing_2_day_revenue",
  "type": "cumulative",
  "type_params": {
    "cumulative_type_params": {
      "window": { "count": 2, "granularity": "day" },
      "grain_to_date": null
    },
    "metric_aggregation_params": {
      "semantic_model": "orders",
      "agg": "sum",
      "expr": "amount",
      "agg_time_dimension": "ordered_at"
    }
  }
}
```

Use a cumulative metric when the question is about running totals or rolling
windows rather than one bucket at a time.

The current compiler evaluates cumulative metrics against a bounded time spine.
That means a cumulative query can emit trailing time buckets that were not
present as raw source dates, as long as those buckets are still needed to
evaluate the rolling window.

## Authoring Checklist

When a metric misbehaves, the most common causes are:

- The semantic model or `semantic_model` name does not match.
- The `agg_time_dimension` is missing or points at the wrong time dimension.
- A filter uses a dimension reference that does not exist.
- A derived or ratio metric references an input metric with the wrong name.
- A cumulative metric uses a window that does not match the intended business
  meaning.

## Working DuckDB Examples

The docs use these query specs:

- [`examples/queries/order_count_by_status.json`](examples/queries/order_count_by_status.json)
- [`examples/queries/revenue_by_day.json`](examples/queries/revenue_by_day.json)
- [`examples/queries/average_order_value.json`](examples/queries/average_order_value.json)
- [`examples/queries/completion_rate_by_day.json`](examples/queries/completion_rate_by_day.json)
- [`examples/queries/trailing_2_day_revenue_by_day.json`](examples/queries/trailing_2_day_revenue_by_day.json)

Those examples are also exercised by the DuckDB test added alongside this doc
work so the examples do not drift silently.
