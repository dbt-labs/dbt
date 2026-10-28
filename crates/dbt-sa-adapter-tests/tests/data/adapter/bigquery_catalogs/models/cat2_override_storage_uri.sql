{# Test overriding 'storage_uri' adapter property should take precedence over everything else. #}
{{ config (
  materialized = 'table',
  catalog_name = "cat2",
  base_location_subpath = 'subpath1',
  base_location_root = 'root2',
  adapter_properties = {
    'base_location_root': 'root3',
    'base_location_subpath': 'subpath2',
    'storage_uri': 'gs://dbt-ci-iceberg-test/successful-override/' ~ target.schema
  }
) }}

select
  "cool_name" as name,
  1 as id
