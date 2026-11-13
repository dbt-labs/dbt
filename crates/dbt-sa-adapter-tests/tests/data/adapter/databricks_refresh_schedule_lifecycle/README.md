# Databricks refresh schedule lifecycle

This test covers the `EVERY` and `ON UPDATE` schedule support ported from
[dbt-databricks #1434](https://github.com/databricks/dbt-databricks/pull/1434).
`EVERY` refreshes on a fixed interval; `ON UPDATE` refreshes when source data
changes, with an optional rate limit. Users can change or remove these schedules
on existing materialized views and streaming tables.

The test follows the same two models through a series of dbt runs. This checks
that Databricks accepts the generated SQL and that each run leaves the expected
schedule in place. Unit tests separately cover parsing, validation, comparison,
and the streaming-table decision to skip manual refresh when an automatic
schedule is unchanged.

| Step | What changes | What it checks |
| --- | --- | --- |
| Setup | Create a small Delta source table with row tracking enabled. | Both models have a shared input. |
| Create | Create a materialized view with `EVERY 1 DAY` and a streaming table with `ON UPDATE`, limited to once every 15 minutes. | Both new schedule types work when creating models. |
| Swap | Switch the view to ON UPDATE and the table to EVERY. | Both schedule types can replace an existing schedule. |
| Change a property | Change only the streaming table's `refresh_test_marker` from `baseline` to `changed`. | The property update is applied and the existing EVERY schedule survives. The marker is an arbitrary test property. |
| Switch to CRON | Give both models a CRON schedule without an explicit timezone. | The new modes can transition back to CRON with the default timezone. |
| Remove schedules | Set both schedule configurations to `{}`. | Both models return to manual refresh. |

After each model run, `assert_refresh_schedules` reads the `Refresh Schedule` row
from `DESCRIBE EXTENDED` and compares it with the expected value. It also checks
that both models succeeded and that the streaming table has the expected marker
property. Expected values account for server formatting, such as reporting
`15 MINUTES` as `900 SECOND` or `1 DAY` as `1 DAYS`.

The test checks schedule configuration, not whether a background refresh fires
at the right time. It does not change source data or wait for scheduled jobs.

The Rust test is `adapters_databricks_refresh_schedule_lifecycle`. Its six steps
map to `run_0` through `run_5`; `_project_overrides` contains the configurations
for subsequent runs. Recording executes against Databricks, while replay uses
the saved responses. For recording and replay commands, see the
[adapter testing guide](../../../../README.md). The harness supplies the
connection profile; the Databricks endpoint must support both model types and
both new schedule types.
