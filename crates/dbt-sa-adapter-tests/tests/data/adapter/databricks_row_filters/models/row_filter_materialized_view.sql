{{ config(materialized='materialized_view') }}

select 'user1' as user_id, 'region_a' as region, cast(100 as bigint) as amount
