{{ config(
    materialized='dynamic_table',
    snowflake_warehouse='FUSION_SLT_WAREHOUSE',
    refresh_mode='CUSTOM_INCREMENTAL',
    incremental_strategy='merge',
    unique_key='order_id',
) }}

-- Reads the dynamic table itself through this, which a CI DT body must write as SELF.
select o.order_id, o.amount
from {{ changes(source('raw', 'orders'), 'default') }} as o
where o.order_id not in (select order_id from {{ this }})
