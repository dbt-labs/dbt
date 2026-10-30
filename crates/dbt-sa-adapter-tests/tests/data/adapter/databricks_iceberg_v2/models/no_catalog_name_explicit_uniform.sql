{{
  config(
    materialized="table",
    table_format="iceberg",
    use_uniform=true,
  )
}}

select 1 as id, 'no_catalog_name_explicit_uniform' as catalog_type
