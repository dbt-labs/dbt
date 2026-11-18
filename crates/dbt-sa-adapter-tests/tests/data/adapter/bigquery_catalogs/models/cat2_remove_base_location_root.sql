{# Test overriding 'base_location_root' with an empty string should reset root to '_dbt'. #}
{{ config (
  materialized = 'table',
  catalog_name = "cat2",
  adapter_properties = {
    'base_location_root': '',
  }
) }}

select
  "cool_name" as name,
  1 as id
