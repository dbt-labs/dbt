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
`--table-kind permanent` or `--table-kind transient` for one variant.

Scenarios that inspect failed candidates explicitly set `wap_retain_failed: true`.
Cleanup scenarios leave the setting unspecified to verify default deletion,
with a separate case exercising explicit `false`.

Each variant checks:

- An error-severity audit fails with explicit `wap_retain_failed: true`; the
  public sentinel stays `[999]`, the existing downstream sentinel stays `[777]`,
  and the candidate retains `[-1, 2]`.
- Reusing that failed build's invocation ID is rejected before staging, even with
  otherwise-passing model SQL and default failed-table cleanup. The retained
  candidate and both sentinels remain unchanged, and the failed command reports
  no successful candidate creation.
- A subsequent standalone `dbt test` passes against the public sentinel, despite
  the retained failing candidate and compiled SQL from the preceding build.
- `dbt retry` creates a different candidate containing `[2, 2]`. The previously
  passing uniqueness audit must now fail, preserving both sentinels. A subsequent
  retry reruns all three audits, publishes `[2, 3]`, and builds the downstream
  through its ordinary public `ref`.
- A deliberate SQL conversion error runs on Snowflake with static analysis off.
  The model errors, all audits and downstream execution are skipped, and both
  existing sentinel tables retain their original rows. Its retained working
  clone still contains the public sentinel `[999]`, proving that cloning preceded
  the failed transformation. The model result must
  include the deliberate conversion-error marker, so an earlier configuration
  or permissions error cannot satisfy this scenario.
- A warning-severity audit also prevents publication, preserves both existing
  sentinel tables, and retains its candidate.
- A singular audit with a literal NULL failure calculation errors instead of
  passing. Both sentinels remain unchanged and the candidate is retained. The
  result must identify the invalid audit result, so an unrelated error cannot
  satisfy this scenario.
- Silencing that warning with `--warn-error-options '{"silence":["LogTestResult"]}'`
  still prevents publication. The audit's displayed status is `pass` with one
  failing row, the model errors, and both sentinel tables remain unchanged.
- A transformation reading `{{ this }}` with static analysis off reads the
  initial working clone, adds one to its sentinel, publishes `[1000]`, and drops
  the successful candidate. This checks runtime execution, not compile-time
  introspection.
- A deferred post-hook changes candidate rows from `[1, 2]` to `[2, 3]` before
  audits; both the published and downstream tables receive `[2, 3]`.
- A post-hook instead changes those rows to `[-1, 2]`, causing the audit to fail.
  Both existing sentinel tables remain unchanged and the retained candidate
  contains the hook's mutation.
- A model SQL header sets a session variable consumed by the CTAS on Snowflake,
  with static analysis off. Both public and downstream tables receive `[41]`.
- A successful first build publishes exactly `[1, 2]` and drops its candidate.
- A failing first build leaves the public table absent.
- By default, audit failures and transformation errors remove
  their confirmed-created candidates after execution finishes, while preserving
  both sentinels. A failing first build also removes its candidate and leaves
  both public tables absent. Checks require the exact removal message and actual
  warehouse absence. An additional audit failure with explicit
  `wap_retain_failed: false` verifies the same cleanup behavior.

Warehouse checks verify the actual permanent/transient table kind, as well as
rows. The default two-kind run also checks permanent-to-transient publication
and rejects transient-to-permanent conversion before a candidate is created.

Assertions check actual warehouse contents, `run_results.json`, and compiled
downstream SQL. Each model must have one canonical result. Audit SQL includes
both generic `not_null`/`unique` tests and a singular test. The runner requires
their exact audit IDs from the fixture manifest, so unrelated passing tests
cannot satisfy the check. Builds use four threads
and do not use fail-fast.

Every staging message must identify its candidate in the same database and
schema as the public model. The runner checks fully qualified names, including
quoted names containing dots or embedded quotes, before recording ownership.
A candidate in another database or a scratch schema fails the check and is not
added to the cleanup inventory. The fixture never creates or drops a schema.
These names isolate ordinary `ref()` calls; schema grants can still allow other
users to access the working tables directly.

Every run uses a copied temporary project and UUID-based public aliases in the
target schema. The runner first verifies those public aliases are absent. It
records candidate ownership only when WAP confirms successful working-table
creation and the current run's invocation ID agrees. A staging-intent message,
failed create-only claim, pre-existing collision reported as “retained if
created,” or foreign invocation never establishes ownership. A matching confirmed
creation event still establishes ownership if the command later fails before
writing its result artifact. It never scans or deletes tables using a wildcard.

The runner's final cleanup drops only its recorded public tables and exact
candidate identifiers. `--keep-objects` skips this final cleanup for inspection;
it does not override default model cleanup. Set `wap_retain_failed: true` to
retain failed candidates; otherwise dbt attempts cleanup after ordinary failed
execution. Interrupted runs and uncertain
publication outcomes retain candidates for investigation. Logs,
per-command artifacts, saved retry state, and `owned_objects.json` stay in the
temporary directory printed at startup. The runner forces text info logging and
JSON artifacts so inherited output settings cannot hide ownership evidence;
it also resets inherited warning overrides so each scenario controls its verdicts
and assigns a fresh invocation UUID to each command, overriding any inherited ID.
The collision scenario deliberately reuses one known fixture invocation ID.
The runner disables inherited failure storage; required WAP audits count their
query results directly and cannot use shared persistent failure tables.
Profile and authentication environment variables are preserved.
On interruption or incomplete cleanup,
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
They also check inherited and inline hooks retain their order exactly once and
model SQL headers preserve the public manifest identity.
They make no warehouse calls and require a built dbt binary:

```sh
DBT_WAP_TEST_BIN="$PWD/target/debug/dbt" \
  python3 -m unittest discover -s crates/dbt-sa-cli/tests -p test_snowflake_wap_cli.py -v
```
