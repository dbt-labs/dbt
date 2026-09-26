# Snowflake table write-audit-publish

## Summary

Add an opt-in `wap` model configuration for native Snowflake SQL models using
`materialized='table'`. During `dbt build`, clone an existing public table to a
uniquely named working table, transform it, run the model's data tests against it,
and publish it only when every required audit reports `PASS`. The working table
lives in the model's existing database and schema. No extra schema or
schema-creation permission is needed.

```sql
{{ config(materialized='table', wap=true) }}

select id, amount from {{ ref('stg_orders') }}
```

The flag can also be inherited through `dbt_project.yml`:

```yaml
models:
  my_project:
    audited_tables:
      +materialized: table
      +wap: true
```

The default is false. A child model can set `wap=false`. Materialization remains
`table`, and the public model database, schema, alias, manifest identity, and
ordinary downstream `ref()` calls remain unchanged.

Failed working tables are removed by default (`wap_retain_failed=false`). To
retain them for inspection after a failed build, configure the model:

```sql
{{ config(materialized='table', wap=true, wap_retain_failed=true) }}
```

This setting also inherits through `+wap_retain_failed` in `dbt_project.yml`.
Successful publication always attempts to remove its working table. Failed-table
cleanup waits until all scheduled model tasks and audits finish, and only removes
tables this invocation confirmed it created. It does not remove older retained
tables. Cleanup errors are warnings and preserve the original build failure.
Cancellation (including fail-fast), crashes, failed ownership claims, or an
uncertain publication response may leave tables behind even with retention
disabled. This is best-effort cleanup, not an expiration policy.

## Build and publication behavior

1. Validate the selected WAP models and their complete set of enabled audits
   before scheduling warehouse writes. Require at least one enabled data test.
2. Check the live public relation and working-name availability, then clone an
   existing public table to an invocation-specific identifier in the model's
   canonical database and schema. Use create-only SQL without `COPY GRANTS` so a
   name collision cannot overwrite another table. Build the model on that
   working relation with ordinary SQL table construction, contracts,
   documentation, and configured permanent/transient lifecycle. If the public
   table does not exist yet, first claim the working name with create-only SQL
   for an empty table, then rebuild it from model SQL. This closes the gap between
   checking name availability and running replacement SQL, including concurrent
   commands that reuse an explicit invocation ID. A failed claim stops the build.
   After the claim, run the model's pre-hooks, its SQL header and table creation,
   and then its post-hooks before any audits run.
3. Bind this model's audit references to the working relation using an
   execution-local relation override. Do not rewrite SQL text or globally change
   the manifest's model relation. Ordinary model dependencies continue to read
   their published relations.
4. Wait for every required audit. `FAIL`, `WARN`, execution error, cancellation,
   skipped audits, and missing raw results all prevent publication; only `PASS`
   permits it. Assess raw audit results and configured severity independently of
   warning display options: silencing `LogTestResult` cannot permit publication.
   Require exactly one result row with a nonnegative integer failure count and
   non-NULL boolean verdicts. Malformed or NULL results are execution errors,
   never implicit passes. Custom failure calculations must return an integer,
   including zero for an empty input when appropriate.
   [dbt failure calculations](https://docs.getdbt.com/reference/resource-configs/fail_calc)
5. Publish the audited contents with one Snowflake clone statement:

   ```sql
   CREATE OR REPLACE [TRANSIENT] TABLE <published_relation>
   CLONE <working_relation> [COPY GRANTS];
   ```

   The destination must be absent or a supported native table. Never pre-drop
   the published relation to accommodate an incompatible destination type.
6. Reconcile configured production grants, restore configured automatic
   clustering, refresh relation metadata, and release downstream tasks. Drop
   the successful working table after publication completes.

Staging copies the model's execution definition to a new table name and snapshots
the existing public table with `CREATE [TRANSIENT] TABLE ... CLONE ...`. The SQL
transformation rebuilds that working table with `CREATE OR REPLACE TABLE ... AS`;
the initial clone does not make table materialization incremental. At runtime,
`this` names the working relation and can read its cloned rows while rebuilding
it. This does not add compile-time introspection guarantees. If transformation
fails before replacing the clone, its original public rows remain inspectable
when `wap_retain_failed=true`.
The empty first-build claim has a reserved placeholder column; model CTAS replaces
its schema before any audits run. If that transformation fails, the empty claim
is removed by default while the public table stays absent; opting into retention
keeps the empty claim for inspection.

Model pre-hooks and post-hooks retain ordinary Snowflake ordering and inheritance.
They run once during working-table materialization and are not repeated at
publication. Use deferred hook SQL or quoted macro calls so `this` resolves to the
working table when the hook runs:

```sql
{{ config(
    materialized='table',
    wap=true,
    post_hook="delete from {{ this }} where amount is null"
) }}
```

Audits see the table after post-hook changes. A pre-hook, model header, or
post-hook failure prevents publication and downstream execution, with the same
failed-table retention and cleanup behavior as a model failure.

Model `sql_header` retains ordinary materialization behavior, including headers
set during rendering with `set_sql_header`. The header executes in the same
session as table creation; its session state is not guaranteed to be available
to audits or publication, which run as separate tasks. Literal header strings
are not given an additional Jinja rendering pass.

Eager hook expressions such as `post_hook=my_macro(this)` can embed the public
table name during parsing; their SQL is not rewritten. Use
`post_hook="{{ my_macro(this) }}"` for deferred candidate targeting. Hardcoded
public names and ordinary `ref()` calls in hooks still target public relations.
Hooks and headers are trusted SQL: effects outside the working table are not
isolated or rolled back by WAP.

On audit failure, the previous published table remains available and downstream
models are skipped. On a first build, failure leaves the published relation
absent. By default, attempt to remove failed working tables after the task graph
finishes. With `wap_retain_failed=true`, retain them for inspection and report
their exact qualified names. Operators remove any remaining tables when they
are no longer needed.

Publication is atomic for one table: concurrent readers see the old or new
version. Each model publishes independently; a later failure does not undo
earlier successful publications. Snowflake DDL is not part of a transaction
spanning the tests and finalization. An error after a successful clone must
report that publication occurred and stop downstream execution rather than claim
the original table is still published. After confirmed publication, a subsequent
failure removes the working table unless `wap_retain_failed=true`.
If submission fails without a definitive Snowflake response, report that the
publication outcome is unknown and direct the operator to query history.
[Snowflake CREATE TABLE](https://docs.snowflake.com/en/sql-reference/sql/create-table)

## Supported commands and audits

| Command or option | Behavior |
| --- | --- |
| `dbt build` | Builds, audits, and publishes selected WAP models. |
| `dbt run` / `dbt clone` selecting a WAP model | Rejects the operation; use `dbt build`. |
| `dbt test` | Tests published relations; never publishes a retained working table. |
| Parse, compile, listing, documentation | Keep canonical model identities. |
| `dbt retry` of a WAP build | Rebuilds retryable WAP models using fresh working tables and reruns all their required audits, including audits that passed previously. Test-only retries remain test-only. |
| `wap=false` or omitted | Preserves existing behavior. |

All enabled data tests owned by the WAP model must be selected. Excluding an
audit or using indirect selection that omits it produces an actionable error;
selection cannot silently weaken the publication gate. Disabled tests do not
participate. Generic tests use their attached model as owner; singular tests
must depend on only the WAP model. Multi-relation audits owned by the WAP model,
including its relationships tests, are excluded from the first version.
Relationships tests owned by downstream models continue to test the published
relation. Audits must use `ref()` or `builtins.ref()`; hardcoded public table names
cannot be redirected to the working table and are outside this contract.
Existing unit-test execution remains
a prerequisite according to normal build selection; unit tests do not replace
the required data audits. Retry also reruns enabled unit tests for rebuilt WAP
models.

WAP audits execute directly on Snowflake after the working table exists. This
version skips static analysis and test aggregation for those audits, and never
reuses a cached test result. Model analysis retains the canonical relation
identity for downstream schema inference. Filtered partial parsing falls back
to a full parse for WAP so audit discovery includes every enabled test.
SQL audit configuration (`where`, `fail_calc`, `warn_if`, and `error_if`) must use
literal values. Reject dynamically rendered expressions and nondefault SQL
configuration whose original literal cannot be established. Parsing can resolve
a `ref()` in those fields to the public table before working-table overrides
exist. Put relation-dependent expressions in the test query instead, where refs
are rebound at execution. Literal thresholds and failure calculations remain
supported; passing tests still mean only what their configured queries assert.
Some configuration forms, including dictionary-style or conditional `config()`
calls and singular-test YAML properties, do not preserve this origin. For a
singular test's literal override, use a direct top-level keyword call such as
`{{ config(error_if='> 5') }}`. Move relation-dependent logic into
the query regardless of configuration syntax.
Required WAP audits cannot store failure rows in a persistent table or view.
The standard test materialization writes failure storage and then reads it in a
separate query; another test or concurrent invocation can replace that shared
table between those statements and produce a false pass. Reject effective
`store_failures` before staging. To inspect failing rows, disable failure storage
and set `wap_retain_failed=true` on the model before rebuilding. Unrelated tests
may store failures provided their storage cannot overwrite a WAP public or
working table.
Synthetic latest-version pointer views are also checked against those protected
relations before view materialization, including pointers created by ordinary
models in the same build.

Reject `--empty`, sampled builds, local execution, and model
`on_error: continue` for WAP publication. The gate must validate the actual
complete Snowflake table and prevent downstream execution after failure.

## Scope and compatibility

The first version requires a Snowflake target and supports SQL, the built-in
Snowflake table materialization, and native permanent/transient tables. It
excludes incremental, view, dynamic, interactive, external, event, hybrid, and
Iceberg/catalog-linked tables; Python models; custom table or test
materializations; and audit `sql_header`. The model and profile quoting settings
must resolve the working table to the same database and schema. Reject a mismatch
before staging, because the built-in table macro creates its relation with the
profile's quoting policy.
Check other model outputs, failure-storage tables, and generated pointer views
under that effective quoting policy too. A distinct manifest spelling must not
allow an ordinary materialization to overwrite a protected WAP relation.
Reject configured `row_access_policy`, `table_tag`, `copy_tags=true`, and custom
table/column constraints in this first version. Ordinary dbt selection tags and
query tags are unaffected. This feature does not preserve arbitrary properties
manually attached to the previous published table.

Project and package macros remain trusted executable code. Requiring built-in
materializations does not sandbox the helpers they dispatch: custom DDL helpers
must honor the supplied relation, and audit helpers must query the working table
and return honest results. A helper that writes to a hardcoded public relation,
or derives it from the canonical model name, can bypass isolation. WAP does not
make arbitrary `run_query` calls or project-level hooks safe.

Suppress explicit model grants on the working table. At replacement,
`copy_grants=true` preserves existing destination grants. On first publication,
omit `COPY GRANTS` so production future grants apply instead of copying working
table grants. Apply configured grants to the published relation afterward.
Because working tables occupy the same schema, existing future grants may make
them visible to other roles. Their names separate them from the public model;
they are **not private or hidden tables**.

Cloning is inexpensive because it initially shares existing storage, but the
transformation, tests, retained failed working tables, and historical data still
incur costs. Cloning suspends automatic clustering, so configured clustering
must be restored after publication. Streams on replaced tables become stale;
stream continuity and object-identity preservation are outside this feature.
[Snowflake cloning reference](https://docs.snowflake.com/en/sql-reference/sql/create-clone)

Working and published table lifecycles use the model's configured lifecycle. The
working table is permanent or transient. A session-scoped temporary table would
require staging, all audits, and publication to use the same session, while the
current task graph uses independent connections. Snowflake also cannot clone a
temporary table into a permanent target, so temporary candidates would not
support the current publication contract for permanent models.
[Snowflake table types and cloning](https://docs.snowflake.com/en/user-guide/tables-temp-transient#comparison-of-table-types)

Disconnecting a client does not necessarily terminate its Snowflake session, so
invocation completion alone would not guarantee temporary-table cleanup.
[Snowflake temporary-table storage](https://docs.snowflake.com/en/user-guide/tables-storage-considerations#temporary-tables)

Retention configuration controls cleanup; it does not change the table lifecycle.
An existing transient public table cannot be cloned into a permanent working table;
reject that configuration before creating the working table. An existing
permanent table may be cloned to either lifecycle. A first build supports either
lifecycle because no initial clone is required.
[Snowflake table design considerations](https://docs.snowflake.com/en/user-guide/table-considerations)

Separate invocations use different working identifiers. This does not provide a
cross-invocation publication lock: users must serialize builds that target the
same model if publication order matters.
An explicitly reused invocation ID can collide with a retained working table;
create-only claims reject that collision rather than reuse or overwrite it.
Working tables must not be modified outside their owning build. Publication
clones the table's current contents; it does not pin audits and publication to
one immutable historical version. Future grants in the shared schema can allow
other writers, so ref isolation alone cannot enforce this condition.

## Implementation and validation

- Add model-only typed `wap` configuration, project inheritance, manifest
  round-tripping, known-key handling, state comparison, and parser validation.
- Build an invocation-local WAP plan from the full resolved manifest. Preserve
  ordinary selection rules and add a publication dependency after the model's
  audits. Do not report the model successful or submit published state before
  publication completes.
- Clone the existing public table to the working relation, register it in the
  adapter cache, then materialize the copied runtime model there. Use scoped
  audit relation overrides. Reuse the existing Snowflake clone SQL helper for
  publication and the grant machinery. Keep vendored macros and dependency
  manifests unchanged.
- Bypass shortcuts that could skip a new candidate or reuse an audit of a
  different relation. On retry, expand only retryable WAP model IDs to include
  all required audits and unit prerequisites; test-only retries remain test-only.
  Never resume or publish an earlier retained working table.
- Test configuration inheritance and manifest/state compatibility; invalid
  scopes; incomplete audit selection; passing/failing/warning/skipped audits;
  dependency ordering; first publication; preservation of previous data after
  failure; permanent/transient lifecycle; quoting; grants; clustering; retained
  failure tables; retry; and ordinary non-WAP behavior.
- Run focused crate tests and a live Snowflake scenario proving that a failing
  audit leaves sentinel production data unchanged, then that passing audits
  replace it and release downstream models. Real Snowflake validation requires
  credentials and should be reported separately from local test results.

Offline regression tests exercise initial-clone SQL and the shipped Snowflake
publication clone macro with the real runtime config, including lifecycle,
quoted names, and first-build/replacement grant behavior. Publication failure
tests cover audit gating, silenced warnings, missing raw results, clone and
finalization errors, session restoration, retained candidates, and cleanup-only
warnings. Ref and compiled-SQL cache tests verify that working-table identities
stay scoped to the current build; a later standalone test reads the public table.
These checks do not establish Snowflake's live DDL or permission behavior.

Further regression coverage executes initial cloning and the shipped table
materialization against a recording adapter, requiring the clone before the
working-table CTAS when a public table exists and checking clone and transformation
error propagation. Scheduler tests drive the production graph, readiness, and
failure propagation with scripted task completions, including delayed audits and
chained WAP models.
Offline CLI tests parse actual projects and inspect manifests using a profile
that cannot connect to Snowflake. The live runner additionally checks that both
existing public and downstream sentinel data survive transformation errors,
audit failures, and warnings, including silenced warnings. With retention enabled,
failed transformations preserve the initial clone's sentinel rows. Separate audit,
transformation, and first-build failures verify default candidate deletion; an
explicit `wap_retain_failed=false` case verifies the same cleanup. A runtime
`this` self-read with static analysis off verifies that transformation can read
the cloned rows.

The live acceptance runner and its invocation instructions are in
[`crates/dbt-sa-cli/tests/data/snowflake_wap/README.md`](../crates/dbt-sa-cli/tests/data/snowflake_wap/README.md).
