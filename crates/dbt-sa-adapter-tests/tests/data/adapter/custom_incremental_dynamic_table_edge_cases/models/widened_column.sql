{{ config(
    materialized='dynamic_table',
    snowflake_warehouse='FUSION_SLT_WAREHOUSE',
    refresh_mode='CUSTOM_INCREMENTAL',
    incremental_strategy='merge',
    unique_key='order_id',
) }}

-- Discovery declared note as the source's varchar(8). After the source widens it, a longer value
-- fails the refresh (002742, too long) until --full-refresh (or CREATE OR ALTER, flag on).
select o.order_id, o.note
from {{ changes(source('raw', 'notes'), 'default') }} as o
qualify row_number() over (
    partition by o.order_id
    order by case when o.METADATA$ACTION = 'INSERT' then 0 else 1 end
) = 1
