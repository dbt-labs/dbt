# dbt_macro_assets
All adapter macros are currently maintained in:
* [dbt-labs/dbt-adapters](https://github.com/dbt-labs/dbt-adapters)
* [databricks/dbt-databricks](https://github.com/databricks/dbt-databricks)

## Changelog

### [2026-09-16]
  - dbt-athena: v1.11.0, vendored from the `dbt-athena` wheel's
    `dbt/include/athena/` (42 files, 2212 SQL LOC). Note the macros ship in the
    `dbt-athena` distribution, NOT `dbt-athena-community`, which is an empty
    shim that merely depends on it.
    No loader change was needed: `internal_package_names` derives the directory
    name as `dbt-{adapter_type}`, and Athena inherits no other adapter's macros.
    `sample_profiles.yml` is not vendored: `is_metadata_file` accepts only
    `dbt_project.yml`, `packages.yml`, `profile_template.yml` and `__init__.py`
    in an internal package, and the sample profile is covered by `dbt init`.

### [2026-08-19]
  - dbt-databricks: view full-refresh precedence from commit 45351e11517d3f37c5ac7a736b5fcba453d3f368

### [2026-03-04]
  - dbt-databricks: v1.11.5 (commit 24325a3195171d36972804e545b2ccf967ab575d)

### [2024-06-10]
  - dbt-databricks: commit ed96fec25ebdcf3f434819b4f15706cebd236a78 (HEAD -> 1.9.latest, origin/1.9.latest)

### [2024-05-19]
  - dbt-adapters: 730ff025e20d79ec26815987b91e4ec48d20910e
  - dbt-snowflake: 13a7ad3f6d0b1fa4b3b05fa238fc6806aaf356eb (excluding `catalog_relations` support)
