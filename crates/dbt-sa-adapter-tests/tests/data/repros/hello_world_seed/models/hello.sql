{{ config(materialized='table') }}

select * from {{ ref('p') }}
