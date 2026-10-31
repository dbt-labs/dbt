{{
  config(
    materialized='table',
    catalog_name='c1',
    copy_grants=true,
  )
}}

select 1 as id
