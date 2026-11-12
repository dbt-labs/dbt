{{ config(
    materialized='dynamic_table',
    snowflake_warehouse='FUSION_SLT_WAREHOUSE',
    refresh_mode='CUSTOM_INCREMENTAL',
    incremental_strategy='merge',
    unique_key='order_id',
    delete_condition='DBT_INTERNAL_SOURCE.is_deleted',
) }}

-- An update arrives as a DELETE and an INSERT of the same key; keep the INSERT.
select
    o.order_id,
    o.customer_id,
    o.amount,
    o.METADATA$ACTION = 'DELETE' and not o.METADATA$ISUPDATE as is_deleted
from {{ changes(source('raw', 'orders'), 'default') }} as o
qualify row_number() over (
    partition by o.order_id
    order by case when o.METADATA$ACTION = 'INSERT' then 0 else 1 end
) = 1
