{{ config(materialized='table', use_safer_relation_operations=true) }}

select 'user1' as user_id, 'region_a' as region, cast(100 as bigint) as amount
