# clickhouse_basic_replay

A single `table` model (`select 1 as id, 'hello' as name`), built with
`test_clickhouse_basic_replay` through the same record/replay harness as the other
ClickHouse tests.

The test is `#[ignore]`d because no recording is committed. Recording `build` stores
the response to `select name from system.databases`, which lists every database on
the instance, so record it against an instance (or user) that only sees databases
that are safe to publish:

```bash
cargo xtask test --llm --no-external-deps -u --adapter-record -p dbt-sa-adapter-tests test_clickhouse_basic_replay
```

Then remove the `#[ignore]` and replay without `-u --adapter-record`.
