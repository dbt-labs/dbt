{{
    config(
        materialized='table'
    )
}} 

select * from `bigquery_use_direct_information_schema_views_src`.INFORMATION_SCHEMA.TABLES LIMIT 1

