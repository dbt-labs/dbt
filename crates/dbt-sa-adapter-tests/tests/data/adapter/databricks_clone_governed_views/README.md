Exercises a row-filtered materialized view, a row-filtered streaming table,
and a column-masked streaming table through `dbt clone` and
`dbt clone --full-refresh` (dbt-labs/dbt#16891).

The source aliases end in `_prod`, in the same ephemeral schema as the clones.
The runner retains the production manifest in `state/` before cloning. Both
clone commands must create ordinary views with the filtered or masked rows;
the original objects must still enforce their policies. Before full refresh,
the clone views are replaced with sentinel queries so the final assertions
also verify that full refresh replaces existing clones.

Run the single test `adapters_databricks_clone_governed_views` using the
record/replay commands in the crate README. Recording requires a Unity Catalog
warehouse that supports materialized views and streaming tables, plus
permission to create functions and policies in the test schema. The harness
drops that schema after recording.

Local debug runs need the same parser/Jinja stack size as CI:

```sh
RUST_MIN_STACK=8388608 cargo test --offline -p dbt-sa-adapter-tests --test dbt_sa_adapter_tests_tests adapters_databricks_clone_governed_views
```

Run only this test per `cargo test` invocation, or use `cargo nextest` to
isolate each test in its own process.
