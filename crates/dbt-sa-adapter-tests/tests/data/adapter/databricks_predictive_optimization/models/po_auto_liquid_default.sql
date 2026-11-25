{{ config(materialized='table', auto_liquid_cluster=true) }}

select cast(1 as bigint) as id
