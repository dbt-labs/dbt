{{
  config(
    materialized='table',
    catalog_name='c2',
  )
}}

select 1 as id
