{{ config(
    materialized='dynamic_table',
    snowflake_warehouse='FUSION_SLT_WAREHOUSE',
    refresh_mode='CUSTOM_INCREMENTAL',
    incremental_strategy='merge',
    unique_key='order_id',
) }}

-- No dedup per key: an update arrives as a DELETE and an INSERT of the same order_id.
select o.order_id, o.amount
from {{ changes(source('raw', 'orders'), 'default') }} as o
