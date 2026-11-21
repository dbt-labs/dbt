{{ config(
    materialized='dynamic_table',
    snowflake_warehouse='FUSION_SLT_WAREHOUSE',
    refresh_mode='CUSTOM_INCREMENTAL',
    incremental_strategy='merge',
    unique_key='order_id',
    transient=true,
) }}

-- APPEND_ONLY changes carry inserts only.
select o.order_id, o.amount
from {{ changes(source('raw', 'orders'), 'append_only') }} as o
qualify row_number() over (partition by o.order_id order by o.amount desc) = 1
