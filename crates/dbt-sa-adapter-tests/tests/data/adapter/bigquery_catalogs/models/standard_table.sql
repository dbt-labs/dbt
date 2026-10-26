{# Test that not setting a catalog name at all should not use a bucket #}
{{ config (
    materialized = 'table'
) }}

select
  "cool_name" as name,
  1 as id,
  current_timestamp() as updated_at
