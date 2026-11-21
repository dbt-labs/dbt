{# Test overriding 'connection' adapter property changes the WITH CONNECTION clause. #}
{{ config (
  materialized = 'table',
  catalog_name = "cat2",
  adapter_properties = {
    'connection_id': 'dbt-test-env.us.__default_cloudresource_connection__',
  }
) }}

select
  "cool_name" as name,
  1 as id
