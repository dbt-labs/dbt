# Snowflake WAP live acceptance

This fixture exercises a built dbt binary against an existing Snowflake profile,
database, and schema. It uses Python 3.9+ standard-library modules and the
project's own `run-operation` assertion macros; no Python dbt or Snowflake driver
is needed.

From the repository root:

```sh
python3 crates/dbt-sa-cli/tests/snowflake_wap_acceptance.py \
  --dbt-bin target/debug/dbt \
  --profiles-dir ~/.dbt \
  --profile YOUR_PROFILE \
  --target YOUR_SNOWFLAKE_TARGET
```

The selected role needs its usual warehouse access plus permission to create,
read, replace, and drop tables in the existing target schema. The runner does not
create or change credentials, profiles, databases, or schemas. All warehouse
work is explicitly invoked by this command. Transformation, audit queries, and
retained tables consume ordinary Snowflake resources.

The default run covers both permanent and transient WAP tables. Use
`--table-kind permanent` or `--table-kind transient` for one variant. Each variant
checks:

- An error-severity audit fails; the public sentinel stays `[999]`, the existing
  downstream sentinel stays `[777]`, and the candidate retains `[-1, 2]`.
- A subsequent standalone `dbt test` passes against the public sentinel, despite
  the retained failing candidate and compiled SQL from the preceding build.
- `dbt retry` creates a different candidate, reruns all three audits including
  the two that previously passed, publishes `[2, 3]`, and builds the downstream
  through its ordinary public `ref`.
- A deliberate SQL conversion error runs on Snowflake with static analysis off.
  The model errors, all audits and downstream execution are skipped, and both
  existing sentinel tables retain their original rows. The model result must
  include the deliberate conversion-error marker, so an earlier configuration
  or permissions error cannot satisfy this scenario.
- A warning-severity audit also prevents publication, preserves both existing
  sentinel tables, and retains its candidate.
- A successful first build publishes exactly `[1, 2]` and drops its candidate.
- A failing first build leaves the public table absent.

Assertions check actual warehouse contents, `run_results.json`, and compiled
downstream SQL. Each model must have one canonical result. Audit SQL includes
both generic `not_null`/`unique` tests and a singular test. Builds use four threads
and do not use fail-fast.

Every staging message must identify its candidate in the same database and
schema as the public model. The runner checks fully qualified names, including
quoted names containing dots or embedded quotes, before recording ownership.
A candidate in another database or a scratch schema fails the check and is not
added to the cleanup inventory. The fixture never creates or drops a schema.

Every run uses a copied temporary project and UUID-based public aliases in the
target schema. The runner first verifies those public aliases are absent. It
records candidate ownership only from WAP's post-preflight staging message, so a
pre-existing collision reported as “retained if created” is never treated as a
table the runner owns. It never scans or deletes tables using a wildcard.

Successful and failed runs clean up only their recorded public tables and exact
candidate identifiers. `--keep-objects` retains them for inspection. Logs,
per-command artifacts, saved retry state, and `owned_objects.json` stay in the
temporary directory printed at startup. On interruption or incomplete cleanup,
use that inventory with `wap_fixture_cleanup(identifiers=...)` and the recorded
variables/profile/target in the copied project. In particular, do not drop every
table whose name begins with `__DBT_WAP_`.

The fixture has no CI credential assumptions and is not part of ordinary local
unit tests. A successful offline parse or unit-test run does not count as this
live acceptance test passing.

The runner's offline unit tests use mocked subprocesses and never load profiles
or make warehouse calls:

```sh
python3 -m unittest discover -s crates/dbt-sa-cli/tests -p test_snowflake_wap_acceptance.py
```

Separate CLI integration checks parse temporary projects with an unusable
Snowflake profile. They verify actual config inheritance and validation,
canonical manifest identities and refs, and repeated parsing when WAP changes.
They make no warehouse calls and require a built dbt binary:

```sh
DBT_WAP_TEST_BIN="$PWD/target/debug/dbt" \
  python3 -m unittest discover -s crates/dbt-sa-cli/tests -p test_snowflake_wap_cli.py -v
```
