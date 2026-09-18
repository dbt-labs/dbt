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

- An error-severity audit fails; the public sentinel stays `[999]`, the candidate
  retains `[-1, 2]`, and the downstream table is not created.
- A subsequent standalone `dbt test` passes against the public sentinel, despite
  the retained failing candidate and compiled SQL from the preceding build.
- `dbt retry` creates a different candidate, reruns all three audits including
  the two that previously passed, publishes `[2, 3]`, and builds the downstream
  through its ordinary public `ref`.
- A warning-severity audit also prevents publication and retains its candidate.
- A successful first build publishes exactly `[1, 2]` and drops its candidate.
- A failing first build leaves the public table absent.

Assertions check actual warehouse contents, `run_results.json`, and compiled
downstream SQL. Each model must have one canonical result. Audit SQL includes
both generic `not_null`/`unique` tests and a singular test. Builds use four threads
and do not use fail-fast.

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
