{{ config(
    materialized='dynamic_table',
    snowflake_warehouse='FUSION_SLT_WAREHOUSE',
    refresh_mode='CUSTOM_INCREMENTAL',
    incremental_strategy='merge',
    unique_key='order_id',
    transient=true,
) }}

-- A transient CI DT: `create transient dynamic table (...) refresh using (...)`.
select o.order_id, o.amount
from {{ changes(source('raw', 'orders'), mode='append_only') }} as o
qualify row_number() over (partition by o.order_id order by o.amount desc) = 1
