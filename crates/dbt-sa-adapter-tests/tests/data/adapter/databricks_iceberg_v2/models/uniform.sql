{{
  config(
    materialized="table",
    catalog_name="e2e",
    table_format="iceberg",
    file_format="delta",
    use_uniform=true,
  )
}}

select 1 as id, 'uniform' as catalog_type
