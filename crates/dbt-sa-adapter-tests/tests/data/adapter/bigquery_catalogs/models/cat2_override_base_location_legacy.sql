{# Test overriding 'base_location_*' at legacy level without adapter properties should override 'base_location_root' in cat2. #}
{{ config (
  materialized = 'table',
  catalog_name = "cat2",
  base_location_subpath = 'subpath1',
  base_location_root = 'root2',
) }}

select
  "cool_name" as name,
  1 as id
