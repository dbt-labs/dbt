## BigQuery materialized views

This test consists of a source `orders.sql` model and a derived `mat_view` on top of it.

What it does is:
1. Build the materialized view with `enable_refresh=true` and `refresh_interval_minutes=60.0`.
2. Pull down the remote status of the materialized view using `dbt run-operation pull_materialized_view_options`
3. Manually update the materialized view using `dbt run-operation update_materialized_view`
4. Pull down the remote status again and verify it was applied
5. Build again
6. Pull down the status again and verify it was reset

