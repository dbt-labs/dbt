# Snowflake table write-audit-publish

## Summary

Add an opt-in `wap` model configuration for native Snowflake SQL models using
`materialized='table'`. During `dbt build`, compute a uniquely named working
table, run the model's data tests against it, and publish it only when every
required audit reports `PASS`. The working table lives in the model's existing
database and schema. No extra schema or schema-creation permission is needed.

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

## Build and publication behavior

1. Validate the selected WAP models and their complete set of enabled audits
   before scheduling warehouse writes. Require at least one enabled data test.
2. Build a working table with an invocation-specific identifier in the model's
   canonical database and schema. Preserve ordinary SQL table construction,
   contracts, documentation, and configured permanent/transient lifecycle.
3. Bind this model's audit references to the working relation using an
   execution-local relation override. Do not rewrite SQL text or globally change
   the manifest's model relation. Ordinary model dependencies continue to read
   their published relations.
4. Wait for every required audit. `FAIL`, `WARN`, execution error, cancellation,
   and skipped audits all prevent publication; only `PASS` permits it.
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

On audit failure, the previous published table remains available and downstream
models are skipped. On a first build, failure leaves the published relation
absent. Retain failed working tables for inspection and report their exact
qualified names. Operators remove them when they are no longer needed.

Publication is atomic for one table: concurrent readers see the old or new
version. Each model publishes independently; a later failure does not undo
earlier successful publications. Snowflake DDL is not part of a transaction
spanning the tests and finalization. An error after a successful clone must
report that publication occurred, retain the working table, and stop downstream
execution rather than claim the original table is still published.
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
| `dbt retry` of a WAP build | Rebuilds a fresh working table and reruns all required audits, including audits that passed previously. |
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
Tests that store failure rows may use their normal failure tables, provided those
relations do not collide with a WAP public table or working table.

Reject `--empty`, sampled builds, local execution, and model
`on_error: continue` for WAP publication. The gate must validate the actual
complete Snowflake table and prevent downstream execution after failure.

## Scope and compatibility

The first version requires a Snowflake target and supports SQL, the built-in
Snowflake table materialization, and native permanent/transient tables. It
excludes incremental, view, dynamic, interactive, external, event, hybrid, and
Iceberg/catalog-linked tables; Python models;
custom materializations; effective model pre/post hooks; and `sql_header`,
including headers added during rendering. The model and profile quoting settings
must resolve the working table to the same database and schema. Reject a mismatch
before staging, because the built-in table macro creates its relation with the
profile's quoting policy.
Reject configured `row_access_policy`, `table_tag`, `copy_tags=true`, and custom
table/column constraints in this first version. Ordinary dbt selection tags and
query tags are unaffected. This feature does not preserve arbitrary properties
manually attached to the previous published table.

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

Working and published table lifecycles must agree: a transient working table
cannot be cloned into a permanent published table.
[Snowflake table design considerations](https://docs.snowflake.com/en/user-guide/table-considerations)

Separate invocations use different working identifiers. This does not provide a
cross-invocation publication lock: users must serialize builds that target the
same model if publication order matters.

## Implementation and validation

- Add model-only typed `wap` configuration, project inheritance, manifest
  round-tripping, known-key handling, state comparison, and parser validation.
- Build an invocation-local WAP plan from the full resolved manifest. Preserve
  ordinary selection rules and add a publication dependency after the model's
  audits. Do not report the model successful or submit published state before
  publication completes.
- Materialize a copied runtime model targeting the working relation, and use
  scoped audit relation overrides. Reuse the existing Snowflake clone SQL
  helper and grant machinery. Keep vendored macros and dependency manifests
  unchanged.
- Bypass shortcuts that could skip a new candidate or reuse an audit of a
  different relation. On retry, expand only the failed/skipped WAP models or
  owners of failed audits to include all required audits and unit prerequisites.
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

Offline regression tests exercise the shipped Snowflake clone macro with the real
runtime config, including lifecycle, quoted names, and first-build/replacement
grant behavior. Publication failure tests cover audit gating, clone and
finalization errors, session restoration, retained candidates, and cleanup-only
warnings. Ref and compiled-SQL cache tests verify that working-table identities
stay scoped to the current build; a later standalone test reads the public table.
These checks do not establish Snowflake's live DDL or permission behavior.

The live acceptance runner and its invocation instructions are in
[`crates/dbt-sa-cli/tests/data/snowflake_wap/README.md`](../crates/dbt-sa-cli/tests/data/snowflake_wap/README.md).
