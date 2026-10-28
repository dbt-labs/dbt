{{ config(materialized='incremental') }}

select 'user1' as user_id, 'region_a' as region, cast(100 as bigint) as amount
