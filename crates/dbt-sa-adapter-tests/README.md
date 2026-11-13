# Adapter record/replay tests

End-to-end tests that run `dbt` commands on small projects against a real
warehouse. Every query and its result are **recorded** once; afterwards the
tests **replay** the recordings offline, with no warehouse or credentials.

## Layout

```
tests/
  adapter/<adapter>.rs     adapter method and feature tests
  repros/<adapter>.rs      regression tests, one per reported bug
  common/mod.rs            how the tests run `dbt` (FS_RUNNER)
  data/
    adapter/<feature>/     a dbt project, usually shared by every adapter
      golden/<adapter>/    its recordings for one adapter
    repros/<repro>/        a dbt project reproducing one bug
      golden/<adapter>/
  golden/
    adapter/<feature>/<test_name>.stdout|.stderr   expected dbt output
    repros/<repro>/<test_name>.stdout|.stderr
```

Choose the module by what the test is for:

- **`adapter`**: an adapter method (`get_relation`, `create_schema`, ...) or a
  feature (materializations, configs, catalogs, ...). Reuse the project in
  `tests/data/adapter/<feature>/` when another adapter already has one.
- **`repros`**: a reported bug, so it stays fixed. Link the issue in the
  test's doc comment and give it its own `tests/data/repros/<repro>/`.

Recordings live in `golden/<adapter>/` as `recordings.db` (SQLite, what replay
reads) and `recordings.yaml` (the same queries, readable, for review). A test
running several commands gets one `run_<N>/` per command, and one
`<test_name>_<N>.stdout|.stderr` goldie per command after the first.

## Setup

Install [cargo-nextest](https://nexte.st). Each test initializes `dbt`'s
tracing, which works once per process, so run more than one test with
`cargo nextest`, not `cargo test`.

Replaying needs no setup. The tests read the `fusion_tests` profile from
`~/.dbt/profiles.yml`, and when that file does not define it, they use the
placeholder profile in
[`dbt-test-utils/profiles/replay_profiles.yml`](../dbt-test-utils/profiles/replay_profiles.yml).

To record, add a `fusion_tests` profile to `~/.dbt/profiles.yml`, with one
output per adapter, named after the adapter type. Start from the placeholder
profile and put real credentials in the output for your adapter. Keep the same
`database` (and, for Snowflake, a warehouse of one of the masked names):
recordings match queries by their SQL text, so the names that end up in that
SQL must be the ones the recordings were made with. The `schema` is replaced
per test.

## Running

| Goal | Command |
| --- | --- |
| Replay every test | `cargo nextest run -p dbt-sa-adapter-tests` |
| Replay one test | `cargo nextest run -p dbt-sa-adapter-tests adapters_snowflake_create_schema` |
| Replay one module | `cargo nextest run -p dbt-sa-adapter-tests -E 'test(/^repros::snowflake::/)'` |
| Update the stdout/stderr goldies from the existing recordings | `GOLDIE_UPDATE=1 cargo nextest run -p dbt-sa-adapter-tests <filter>` |
| Record against the warehouse and update all goldies | `GOLDIE_UPDATE=1 ADAPTER_RECORD=1 cargo nextest run -p dbt-sa-adapter-tests <filter>` |

When recording, each test:

1. runs its commands in a fresh schema named `<test_name>___<timestamp>___`,
2. drops that schema afterwards (only that one: anything a test creates in another
   schema is left behind, so drop it yourself),
3. rewrites the schema name in the recordings to `<test_name>`, which is the
   schema replay runs in.

The existing recordings of a recorded test are deleted first, so record only
the tests you mean to.

Set `DBT_FS_RECORD_NO_YAML=1` to skip writing `recordings.yaml`.

Adapters that are not generally available are refused by release builds;
debug builds (the default for tests) allow them, or set
`DBT_ALLOW_EXPERIMENTAL_ADAPTERS=true`.

## Adding a test

1. Create the project under `tests/data/adapter/<feature>/` or
   `tests/data/repros/<repro>/`, with `profile: fusion_tests` in
   `dbt_project.yml`. Do not commit a `profiles.yml`: the harness writes one
   from `~/.dbt/profiles.yml` only when the project has none.
2. Add the test to `tests/adapter/<adapter>.rs` or `tests/repros/<adapter>.rs`.
   Most need only the file's helper:

   ```rust
   #[dbt_runtime::test]
   async fn adapters_snowflake_create_schema() -> FsResult<()> {
       run_fs_record_replay_snowflake(current_function_name!(), "tests/data/adapter/create_schema")
           .await
   }
   ```

   Otherwise call `run_fs_record_replay_task_seq` directly. Its arguments are,
   in order: the crate root, the runner, the test name, the project path, the
   adapter, a fixed schema (`None` for the per-test one), the commands, extra
   flags appended to each command, and extra environment variables.
3. Record it, then replay it to check that the recording is complete.
4. Review and commit the project, `golden/<adapter>/` and the goldies.

The test name is part of every recorded query (through the schema name), so
renaming a test means recording it again.

### Several commands

Each command is a turn; a turn can first replace project files, to simulate a
change between runs:

```rust
&[
    AdapterRecordReplayTurn::new("run", &[]), // run_0
    AdapterRecordReplayTurn::new(
        "run",
        &[FileOverride {
            original: "dbt_project.yml",                      // in the project
            overriding_file: "_project_overrides/run_1.yml",  // from the project root
        }],
    ), // run_1
]
```

Plain strings (`&["run", "test"]`) work when no turn replaces files.

### Tables the commands don't create

If a test reads relations its commands don't create, make creating them the
first command: a `run-operation` of a macro in the project that creates them in
`target.schema` (see `tests/data/adapter/column_datatypes/macros/seed_snowflake.sql`).
It is recorded and replayed like the other commands, and the schema is dropped
with the rest after recording.

## Reviewing recordings

Review the SQL in `recordings.yaml` and the stdout/stderr goldies.
`recordings.db` changes on every recording, even when the queries don't: its
results carry ids and timestamps, and SQLite's file layout varies between
writes. Its diff is expected.

## Adding an adapter

1. Add an output named after the adapter type to the placeholder profile, with
   placeholder credentials and the database you record with, and to your own
   `fusion_tests` profile.
2. Add `tests/adapter/<adapter>.rs`, and `tests/repros/<adapter>.rs` for its
   bug repros, each with its own `run_fs_record_replay_<adapter>` helper, and
   declare them in the `mod.rs` beside them.
3. Record against existing projects under `tests/data/adapter/` where they
   apply.
