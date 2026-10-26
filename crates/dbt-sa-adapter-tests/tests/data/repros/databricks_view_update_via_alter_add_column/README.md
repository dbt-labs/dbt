# `view_update_via_alter` + `persist_docs.columns` when a column is added

Repro for https://github.com/dbt-labs/dbt-core/issues/15583.

Two turns, mirroring the issue's steps:

```
run   # run_0: create `foobar` as a view over foo_key, foo_name (+ column comments)
run   # run_1: same model with foo_description added (model + schema.yml overridden)
```

Turn 1 takes the `view_update_via_alter` branch, so the changeset drives the update instead of
`CREATE OR REPLACE VIEW`. It must emit, in this order:

```sql
ALTER VIEW ... AS (select '1' as foo_key, 'foo' as foo_name, 'test' as foo_description)
COMMENT ON COLUMN ....`foo_description` IS 'Description of foo'
```

Before the `query` relation config component was implemented
(`fs/sa/crates/dbt-adapter/src/relation/databricks/config/components/query.rs`) the changeset
carried no `query` entry, so `alter_view` skipped the `ALTER VIEW` and the `COMMENT ON COLUMN`
targeted a column the view did not have yet:

```
[UNRESOLVED_COLUMN.WITH_SUGGESTION] A column, variable, or function parameter with name
`foo_description` cannot be resolved. Did you mean one of the following? [`foo_key`, `foo_name`].
```
