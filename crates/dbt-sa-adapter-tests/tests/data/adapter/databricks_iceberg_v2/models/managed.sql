{{
  config(
    materialized="table",
    catalog_name="e2e",
    table_format="iceberg",
  )
}}

select 1 as id, 'managed' as catalog_type
