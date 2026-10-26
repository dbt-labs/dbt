{# Test that connection_id from a v2 catalog is used in the WITH CONNECTION clause. #}
{{ config(
  materialized = 'table',
  catalog_name = 'cat1',
) }}

select
  "cool_name" as name,
  1 as id
