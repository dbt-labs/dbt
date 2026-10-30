# dbt-fusion Issue #498 Reprex

**Column metadata query fails when information_schema table is used as the source for a model**

https://github.com/dbt-labs/dbt-fusion/issues/498

## Prerequisites

Do one of the following:
1. Option 1: Adjust `profile:` within `dbt_project.yml` to be one of your pre-existing profiles and delete or replace `profiles.yml`, or
2. Option 2: Set the environment variables listed within `example.zshrc`.

## Reprex

This issue occurs when using INFORMATION_SCHEMA tables as sources for dbt models. The bug manifests in dbt-fusion's metadata column queries, which generate duplicate INFORMATION_SCHEMA references.

### Model Setup

Created `models/stg_jobs.sql`:
```sql
{{
    config(
        materialized='table'
    )
}} 

select * from {{ this.schema }}.INFORMATION_SCHEMA.TABLES
```

### dbt-core Behavior (Expected)

Run these commands to see how dbt-core behaves:

```shell
dbt-core build --select stg_jobs --log-level debug
```

**Result**: ✅ **SUCCESS** - No metadata query issues

The debug output shows clean metadata queries:
```
select * from dbt_dbeatty.INFORMATION_SCHEMA.TABLES
```

### dbt-fusion Behavior (Bug Reproduction)  

Run these commands to see how dbt-fusion behaves:

```shell
dbt build --select stg_jobs --log-level debug
```

**Result**: ✅ **SUCCESS** but with **BUG CONFIRMED** in metadata queries

The debug output reveals the problematic duplicate INFORMATION_SCHEMA reference:
```
SELECT column_name, data_type, is_nullable FROM dbt_dbeatty.INFORMATION_SCHEMA.INFORMATION_SCHEMA.COLUMNS WHERE table_name = 'TABLES' UNION ALL SELECT * FROM (SELECT '', '', '' FROM dbt_dbeatty.INFORMATION_SCHEMA.TABLES LIMIT 0);
```

Notice the duplicate: `INFORMATION_SCHEMA.INFORMATION_SCHEMA.COLUMNS` 

This matches exactly the issue described in GitHub issue #498 where the metadata query generation incorrectly duplicates the INFORMATION_SCHEMA reference.

### Bug Summary

- **Expected**: `INFORMATION_SCHEMA.COLUMNS` 
- **Actual**: `INFORMATION_SCHEMA.INFORMATION_SCHEMA.COLUMNS` (duplicate)
- **Impact**: In some BigQuery configurations, this causes metadata queries to fail
- **dbt-core**: Works correctly ✅
- **dbt-fusion**: Has the bug ❌
