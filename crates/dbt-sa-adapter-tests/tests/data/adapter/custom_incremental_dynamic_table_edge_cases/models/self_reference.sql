{{ config(
    materialized='dynamic_table',
    snowflake_warehouse='FUSION_SLT_WAREHOUSE',
    refresh_mode='CUSTOM_INCREMENTAL',
    incremental_strategy='merge',
    unique_key='order_id',
) }}

-- Reads the table itself through this, which a refresh body must write as SELF: on a first build,
-- column discovery fails because the table doesn't exist yet (002003).
select o.order_id, o.amount
from {{ changes(source('raw', 'orders'), 'default') }} as o
where o.order_id not in (select order_id from {{ this }})
