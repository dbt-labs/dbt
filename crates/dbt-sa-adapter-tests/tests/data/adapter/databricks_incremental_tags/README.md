# Databricks Incremental Tags Record/Replay Test Suite

This directory contains a test suite that validates how the Databricks adapter handles Databricks-specific tags for dbt models, with a focus on incremental materializations. The suite exercises both the dbt macro layer (where model authors declare `databricks_tags`) and the adapter layer (where tags are fetched from the remote catalog, diffed, and applied). The goal is to ensure tags are created, preserved, and updated predictably across multiple `dbt run` invocations without losing existing tags that are not part of the current model config. This aligns with the behaviour in core, as described in this [issue](https://github.com/databricks/dbt-databricks/issues/1069).


## High-level summary

- dbt tags are metadata maintained inside dbt for selection/documentation. They do not automatically propagate to Databricks objects.
- Databricks tags (exposed in this repo as `databricks_tags`) are properties applied to Databricks tables/views. They are part of the relation configuration the adapter manages.

## Databricks tags vs dbt tags
- dbt tags (the `tags` you add to models) are used by dbt itself. They are part of dbt metadata and are useful for selecting models, grouping, documentation, or orchestrating runs. They are not instructions to the adapter to set properties on the database objects.
- Databricks tags (`databricks_tags` in this project) are a separate, database-level concept: they are properties associated with Databricks tables/views that can be queried from the catalog (for example via `information_schema.tags`) and can be set/altered by the adapter at materialization time. Because they live on the physical relation, they must be managed explicitly.

## Running the tests

Run the specific record/replay test with the project's xtask/test runner:

```sh
cargo xtask test -p dbt-adapter-tests adapters_databricks_incremental_tags
```