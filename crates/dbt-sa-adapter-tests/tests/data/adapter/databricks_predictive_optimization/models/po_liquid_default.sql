{{ config(materialized='table', liquid_clustered_by=['id']) }}

select cast(1 as bigint) as id
