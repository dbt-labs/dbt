{# Test overriding 'base_location_*' adapter properties should take precedence over legacy properties. #}
{{ config (
  materialized = 'table',
  catalog_name = "cat2",
  base_location_subpath = 'subpath1',
  base_location_root = 'root2',
  adapter_properties = {
    'base_location_root': 'root3',
    'base_location_subpath': 'subpath2',
  }
) }}

select
  "cool_name" as name,
  1 as id
