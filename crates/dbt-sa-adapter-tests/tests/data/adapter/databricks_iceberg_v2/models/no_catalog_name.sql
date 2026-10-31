{{
  config(
    materialized="table",
    table_format="iceberg",
  )
}}

select 1 as id, 'no_catalog_name' as catalog_type
