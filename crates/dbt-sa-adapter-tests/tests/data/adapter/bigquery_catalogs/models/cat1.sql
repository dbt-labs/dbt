{# Test that a standard table works with iceberg #}
{{ config (
  materialized = 'table',
  catalog_name = "cat1"
) }}

select
  "cool_name" as name,
  1 as id
