# MetricFlow Test Recordings

Each `<dialect>_recordings.parquet` file is a **parquet tome** that contains all
pre-recorded query results for that dialect. The tome stores each test case as a
row with columns `(name: String, ipc_data: Binary)` where `ipc_data` is an
LZ4-compressed Arrow IPC stream.

## Resolving merge conflicts

These files are binary and cannot be merged. If you hit a conflict on any
`.parquet` tome, resolve it by **re-recording** from the live database:

```sh
cargo test -p dbt-metricflow --test <dialect>_compat -- --ignored --nocapture
```

This overwrites the tome with a fresh recording.

## Recording a new dialect

```sh
# Requires fusion_tests/<dialect> profile in ~/.dbt/profiles.yml
cargo test -p dbt-metricflow --test <dialect>_compat -- --ignored --nocapture
```

## Replaying (default, no database needed)

```sh
cargo test -p dbt-metricflow --test <dialect>_compat
```
