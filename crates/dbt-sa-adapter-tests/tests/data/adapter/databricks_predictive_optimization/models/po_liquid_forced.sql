{{ config(materialized='table', liquid_clustered_by=['id'], skip_optimize=false) }}

select cast(1 as bigint) as id
