Per-model database switching on Redshift via `USE` / `RESET USE`. When a model's `database`
differs from the connection default, the materialization is wrapped with `USE <database>` /
`RESET USE`. Ref: [dbt-labs/dbt-adapters#1787](https://github.com/dbt-labs/dbt-adapters/issues/1787).

Two models cover both branches of Redshift database switching:

- `same_database_model` — `database` = default (`ci`) → no `USE` / `RESET USE`.
- `cross_database_model` — `database` = `ci_alt` → `USE ci_alt` / `RESET USE` around the run.

`datasharing: true` in `_profiles.override.yml` relaxes the cross-database `verify_database`
check; the override inherits the real `fusion_tests` connection.

```
cargo xtask test -u --adapter-record -p dbt-adapter-tests adapters_redshift_cross_database_use
```
