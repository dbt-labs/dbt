{{ config(
    materialized='dynamic_table',
    snowflake_warehouse='FUSION_SLT_WAREHOUSE',
    target_lag='1 hour',
    refresh_mode='CUSTOM_INCREMENTAL',
    incremental_strategy='merge',
    unique_key=['warehouse_id', 'sku'],
    delete_condition='DBT_INTERNAL_SOURCE.is_deleted',
) }}

-- Starts with a CTE, which a refresh body cannot: the merge must wrap it in a subquery.
with stock_changes as (
    select
        s.warehouse_id,
        s.sku,
        s.quantity,
        s.METADATA$ACTION as change_action,
        s.METADATA$ISUPDATE as change_is_update
    from {{ changes(source('raw', 'stock'), 'default') }} as s
)

select
    warehouse_id,
    sku,
    quantity,
    change_action = 'DELETE' and not change_is_update as is_deleted
from stock_changes
qualify row_number() over (
    partition by warehouse_id, sku
    order by case when change_action = 'INSERT' then 0 else 1 end
) = 1
