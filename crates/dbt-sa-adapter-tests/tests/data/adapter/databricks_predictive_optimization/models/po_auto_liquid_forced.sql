{{ config(materialized='table', auto_liquid_cluster=true, skip_optimize=false) }}

select cast(1 as bigint) as id
