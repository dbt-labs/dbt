# Metric Semantics

This document describes the user-visible semantics implemented by the current
`dbt-metricflow` compiler.

The examples in this doc use a small DuckDB-backed orders dataset:

- Full manifest: [`examples/duckdb_orders_manifest.json`](examples/duckdb_orders_manifest.json)
- DuckDB setup SQL: [`examples/duckdb_orders.sql`](examples/duckdb_orders.sql)
- Example query specs: [`examples/queries/`](examples/queries/)

## CLI Query Surface

The CLI is a compiler, not a query runner. `dbt-metricflow compile` takes a
metric query and prints SQL. You then execute that SQL in DuckDB, Snowflake, or
another supported warehouse.

The main command shape is:

```sh
dbt-metricflow compile -m docs/examples/duckdb_orders_manifest.json --metrics order_count
```

The current query surface belongs in this document, not the authoring doc:

- doc 1: what metric queries mean, and how to issue them
- doc 2: how to define metrics in the manifest

### CLI Options

- `-m, --manifest <PATH>`: required path to semantic manifest JSON
- `--metrics <m1,m2,...>`: required comma-separated metric names
- `--group-by <specs>`: comma-separated grouping specs
- `--where <filter>`: repeatable filter; shorthand or full Jinja template
- `--order-by <specs>`: comma-separated sort keys; prefix with `-` for descending
- `--limit <N>`: limit output rows
- `--time-constraint START END`: constrain `metric_time` to a closed range
- `--dialect <name>`: render SQL for `duckdb`, `snowflake`, `redshift`,
  `bigquery`, or `databricks`
- `example`: print a starter manifest with sample queries

### One Example Per Query Feature

```sh
# 0. Print a starter manifest
dbt-metricflow example

# 1. Smallest possible query: one metric, no grouping
dbt-metricflow compile \
  -m docs/examples/duckdb_orders_manifest.json \
  --metrics order_count

# 2. Group by metric_time
dbt-metricflow compile \
  -m docs/examples/duckdb_orders_manifest.json \
  --metrics revenue \
  --group-by metric_time:day

# 3. Group by a plain dimension
dbt-metricflow compile \
  -m docs/examples/duckdb_orders_manifest.json \
  --metrics order_count \
  --group-by status

# 4. Group by an entity
dbt-metricflow compile \
  -m docs/examples/duckdb_orders_manifest.json \
  --metrics order_count \
  --group-by "Entity('customer')"

# 5. Filter with CLI shorthand
dbt-metricflow compile \
  -m docs/examples/duckdb_orders_manifest.json \
  --metrics order_count \
  --where "status=completed"

# 6. Filter with explicit Jinja-style syntax
dbt-metricflow compile \
  -m docs/examples/duckdb_orders_manifest.json \
  --metrics order_count \
  --where "{{ Dimension('status') }} = 'completed'"

# 7. Query multiple metrics together
dbt-metricflow compile \
  -m docs/examples/duckdb_orders_manifest.json \
  --metrics order_count,revenue \
  --group-by metric_time:day

# 8. Sort descending and limit rows
dbt-metricflow compile \
  -m docs/examples/duckdb_orders_manifest.json \
  --metrics revenue \
  --group-by metric_time:day \
  --order-by -metric_time \
  --limit 2

# 9. Apply a time constraint
dbt-metricflow compile \
  -m docs/examples/duckdb_orders_manifest.json \
  --metrics revenue \
  --group-by metric_time:day \
  --time-constraint 2024-01-01 2024-01-02

# 10. Render for another dialect
dbt-metricflow compile \
  -m docs/examples/duckdb_orders_manifest.json \
  --metrics revenue \
  --group-by metric_time:day \
  --dialect snowflake
```

The checked-in JSON specs that correspond to those examples are:

- [`examples/queries/order_count_total.json`](examples/queries/order_count_total.json)
- [`examples/queries/revenue_by_day.json`](examples/queries/revenue_by_day.json)
- [`examples/queries/order_count_by_status.json`](examples/queries/order_count_by_status.json)
- [`examples/queries/order_count_by_customer.json`](examples/queries/order_count_by_customer.json)
- [`examples/queries/order_count_where_completed.json`](examples/queries/order_count_where_completed.json)
- [`examples/queries/multi_metric_by_day.json`](examples/queries/multi_metric_by_day.json)
- [`examples/queries/revenue_by_day_desc_limit_2.json`](examples/queries/revenue_by_day_desc_limit_2.json)
- [`examples/queries/revenue_by_day_time_constraint.json`](examples/queries/revenue_by_day_time_constraint.json)

The two filter examples differ only at the CLI layer. The shorthand
`status=completed` is expanded to the same internal where-filter form as:

```text
{{ Dimension('status') }} = 'completed'
```

## Query Walkthroughs

This section uses the exact shape we want readers to internalize:

1. CLI call
2. What this query means
3. Generated DuckDB SQL
4. Short note on why the SQL looks like that

### Single Metric, No Grouping

CLI call:

```sh
dbt-metricflow compile \
  -m docs/examples/duckdb_orders_manifest.json \
  --metrics order_count
```

What this query means:

Return one row for the whole dataset, with the total number of orders.

Generated DuckDB SQL:

```sql
SELECT
  SUM(1) AS order_count
FROM "main"."orders" AS o
```

Short note:

`order_count` is a simple metric whose measure expression is `1`, so the
compiler emits a single aggregate over the base semantic model with no
`GROUP BY`.

Checked-in SQL: [`examples/sql/order_count_total.sql`](examples/sql/order_count_total.sql)

### Group By `metric_time`

CLI call:

```sh
dbt-metricflow compile \
  -m docs/examples/duckdb_orders_manifest.json \
  --metrics revenue \
  --group-by metric_time:day
```

What this query means:

Return one row per day, with daily revenue.

Generated DuckDB SQL:

```sql
SELECT
  DATE_TRUNC('day', o.ordered_at)::DATE AS metric_time,
  SUM(o.amount) AS revenue
FROM "main"."orders" AS o
GROUP BY 1
```

Short note:

`metric_time` resolves to the metric's `agg_time_dimension`, which is
`ordered_at` here. The compiler truncates it to day grain and groups on that
derived column.

Checked-in SQL: [`examples/sql/revenue_by_day.sql`](examples/sql/revenue_by_day.sql)

### Group By A Plain Dimension

CLI call:

```sh
dbt-metricflow compile \
  -m docs/examples/duckdb_orders_manifest.json \
  --metrics order_count \
  --group-by status
```

What this query means:

Return one `order_count` per status value.

Generated DuckDB SQL:

```sql
SELECT
  o.status AS status,
  SUM(1) AS order_count
FROM "main"."orders" AS o
GROUP BY 1
```

Short note:

Because `status` is a dimension on the same semantic model as the metric, the
compiler can project the base column directly and group on it.

Checked-in SQL: [`examples/sql/order_count_by_status.sql`](examples/sql/order_count_by_status.sql)

### Group By An Entity

CLI call:

```sh
dbt-metricflow compile \
  -m docs/examples/duckdb_orders_manifest.json \
  --metrics order_count \
  --group-by "Entity('customer')"
```

What this query means:

Return one `order_count` per customer entity.

Generated DuckDB SQL:

```sql
SELECT
  o.customer_id AS customer,
  SUM(1) AS order_count
FROM "main"."orders" AS o
GROUP BY 1
```

Short note:

Entity grouping resolves to the entity expression from the semantic model. In
this example, `customer` maps to `customer_id`.

Checked-in SQL: [`examples/sql/order_count_by_customer.sql`](examples/sql/order_count_by_customer.sql)

### Filtered Query

CLI call:

```sh
dbt-metricflow compile \
  -m docs/examples/duckdb_orders_manifest.json \
  --metrics order_count \
  --where "status=completed"
```

What this query means:

Return one row for the whole dataset, but only count orders whose status is
`completed`.

Generated DuckDB SQL:

```sql
SELECT
  SUM(1) AS order_count
FROM "main"."orders" AS o
WHERE o.status = 'completed'
```

Short note:

The CLI shorthand `status=completed` is normalized into a dimension filter, and
that filter becomes a regular SQL `WHERE` clause on the resolved column.

Checked-in SQL: [`examples/sql/order_count_where_completed.sql`](examples/sql/order_count_where_completed.sql)

### Multiple Metrics In One Query

CLI call:

```sh
dbt-metricflow compile \
  -m docs/examples/duckdb_orders_manifest.json \
  --metrics order_count,revenue \
  --group-by metric_time:day
```

What this query means:

Return one row per day, with both order count and revenue.

Generated DuckDB SQL:

```sql
SELECT
  DATE_TRUNC('day', o.ordered_at)::DATE AS metric_time,
  SUM(1) AS order_count,
  SUM(o.amount) AS revenue
FROM "main"."orders" AS o
GROUP BY 1
```

Short note:

Because both metrics are simple metrics on the same semantic model with the
same aggregation time dimension, the compiler can emit one shared query instead
of separate metric CTEs.

Checked-in SQL: [`examples/sql/multi_metric_by_day.sql`](examples/sql/multi_metric_by_day.sql)

### Order By And Limit

CLI call:

```sh
dbt-metricflow compile \
  -m docs/examples/duckdb_orders_manifest.json \
  --metrics revenue \
  --group-by metric_time:day \
  --order-by -metric_time \
  --limit 2
```

What this query means:

Return daily revenue, newest day first, capped at two rows.

Generated DuckDB SQL:

```sql
SELECT
  DATE_TRUNC('day', o.ordered_at)::DATE AS metric_time,
  SUM(o.amount) AS revenue
FROM "main"."orders" AS o
GROUP BY 1
ORDER BY metric_time DESC
LIMIT 2
```

Short note:

Ordering and limiting are applied after aggregation. The `-metric_time` CLI
syntax becomes `ORDER BY metric_time DESC`.

Checked-in SQL: [`examples/sql/revenue_by_day_desc_limit_2.sql`](examples/sql/revenue_by_day_desc_limit_2.sql)

### Time Constraint

CLI call:

```sh
dbt-metricflow compile \
  -m docs/examples/duckdb_orders_manifest.json \
  --metrics revenue \
  --group-by metric_time:day \
  --time-constraint 2024-01-01 2024-01-02
```

What this query means:

Return daily revenue, but only for the constrained metric-time range.

Generated DuckDB SQL:

```sql
SELECT
  DATE_TRUNC('day', o.ordered_at)::DATE AS metric_time,
  SUM(o.amount) AS revenue
FROM "main"."orders" AS o
WHERE CAST(o.ordered_at AS TIMESTAMP) >= CAST('2024-01-01' AS TIMESTAMP)
  AND CAST(o.ordered_at AS TIMESTAMP) < CAST('2024-01-02' AS TIMESTAMP) + INTERVAL '1 day'
GROUP BY 1
```

Short note:

The compiler translates the time constraint into a timestamp predicate on the
metric's aggregation time column. The end date is expanded to the end of the
day for day-grain queries.

Checked-in SQL: [`examples/sql/revenue_by_day_time_constraint.sql`](examples/sql/revenue_by_day_time_constraint.sql)

### Cumulative Metric

CLI call:

```sh
dbt-metricflow compile \
  -m docs/examples/duckdb_orders_manifest.json \
  --metrics trailing_2_day_revenue \
  --group-by metric_time:day
```

What this query means:

Return one row per day, where each row is revenue over a rolling two-day
window.

Generated DuckDB SQL:

```sql
WITH
  trailing_2_day_revenue_src AS (
    SELECT f.ordered_at AS src_time, f.amount AS measure_value FROM "main"."orders" AS f
  ),
  trailing_2_day_revenue_spine AS (
    SELECT ds::DATE AS spine_time FROM generate_series((SELECT MIN(src_time) FROM trailing_2_day_revenue_src), (SELECT MAX(src_time) FROM trailing_2_day_revenue_src) + INTERVAL '2 day', INTERVAL '1 day') AS t(ds)
  ),
  trailing_2_day_revenue AS (
    SELECT DATE_TRUNC('day', spine.spine_time) AS metric_time, SUM(src.measure_value) AS trailing_2_day_revenue FROM trailing_2_day_revenue_spine AS spine INNER JOIN trailing_2_day_revenue_src AS src ON src.src_time <= spine.spine_time AND src.src_time > spine.spine_time - INTERVAL '2 day' GROUP BY 1
  )
SELECT *
FROM trailing_2_day_revenue
```

Short note:

Cumulative metrics are more complex because the compiler needs a source CTE, a
generated spine, and a rolling-window join condition. This is why cumulative
queries look structurally different from simple metric queries.

Checked-in SQL: [`examples/sql/trailing_2_day_revenue_by_day.sql`](examples/sql/trailing_2_day_revenue_by_day.sql)

## What A Metric Is

A metric is a named, queryable value.

When you query a metric, the compiler returns one value for each requested
grouping key:

- With no `group_by`, you get one row for the whole filtered dataset.
- With a time grouping, you get one row per time bucket.
- With a dimension grouping, you get one row per dimension value.
- With multiple groupings, you get one row per combination.

In the DuckDB example:

- `order_count` means "how many orders are in scope"
- `revenue` means "the sum of `amount` for the rows in scope"
- `completed_order_count` means "how many orders are in scope after applying the metric's own filter"
- `average_order_value` means "revenue divided by order_count"

## Scope

Every metric is evaluated over a scope. The scope is defined by:

- the metric definition itself
- the query's `where` filters
- the query's `group_by`
- the query's `time_constraint`

That means a metric is not just a stored column. It is a calculation over a
set of rows chosen by the semantic model and by the query.

## `metric_time`

`metric_time` is the default time axis for a metric query.

For simple metrics, it comes from the metric's `agg_time_dimension`. In the
DuckDB example, both `order_count` and `revenue` use `ordered_at` as their
aggregation time dimension, so:

```json
{
  "metrics": ["revenue"],
  "group_by": ["TimeDimension('metric_time', 'day')"]
}
```

returns one row per day. The exact query spec lives at
[`examples/queries/revenue_by_day.json`](examples/queries/revenue_by_day.json).

## Dimensions And Entities

Dimensions slice a metric. They do not change what the metric means; they
change the grain at which it is reported.

For example:

```json
{
  "metrics": ["order_count"],
  "group_by": ["Dimension('status')"]
}
```

produces one `order_count` per order status. The query spec lives at
[`examples/queries/order_count_by_status.json`](examples/queries/order_count_by_status.json).

Entities are the join keys that let the compiler connect semantic models.
They matter most when a query groups by or filters on dimensions that live on
another semantic model.

## Metric Filters vs Query Filters

A metric can have its own filter in the manifest. That filter is part of the
metric's meaning.

In the DuckDB example, `completed_order_count` is defined with:

```json
{
  "where_filters": [
    {
      "where_sql_template": "{{ Dimension('status') }} = 'completed'"
    }
  ]
}
```

That filter applies every time `completed_order_count` is used, including when
it is used inside another metric such as `completion_rate`.

A query-level `where` filter is different. It narrows the current query, but it
does not change the metric definition stored in the manifest.

## Derived, Ratio, And Cumulative Metrics

Not every metric is a direct aggregation from a base table.

- A simple metric aggregates a measure from one semantic model.
- A derived metric combines other metrics with an expression.
- A ratio metric divides one metric by another.
- A cumulative metric evaluates a metric over a rolling or grain-to-date window.

The semantic rule is the same in all cases: the result is still one value per
requested output grain.

For example:

- [`examples/queries/average_order_value.json`](examples/queries/average_order_value.json)
  asks for a derived metric with no grouping, so it returns one row.
- [`examples/queries/completion_rate_by_day.json`](examples/queries/completion_rate_by_day.json)
  asks for a ratio metric by day, so it returns one row per day.
- [`examples/queries/trailing_2_day_revenue_by_day.json`](examples/queries/trailing_2_day_revenue_by_day.json)
  asks for a cumulative metric by day. In the current compiler, cumulative
  queries are evaluated against a bounded spine-backed range, so the output can
  include trailing buckets beyond the raw source dates. Each bucket's value is
  still computed over the rolling window.

## Working Mental Model

The most reliable mental model is:

1. Pick the metric.
2. Apply the metric's own definition and filters.
3. Apply the query's filters and time constraint.
4. Group the result set by the requested dimensions and time buckets.
5. Compute one metric value per output group.

If you keep that model in mind, the rest of the query behavior is usually
predictable.
