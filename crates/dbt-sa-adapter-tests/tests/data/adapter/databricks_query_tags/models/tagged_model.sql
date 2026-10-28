{{ config(
    materialized='table',
    query_tags='{"dbt_test_suite": "databricks_query_tags"}'
) }}

select 1 as id
