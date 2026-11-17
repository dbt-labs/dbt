{{ config(
    materialized='dynamic_table',
    snowflake_warehouse='FUSION_SLT_WAREHOUSE',
    refresh_mode='CUSTOM_INCREMENTAL',
    incremental_strategy='merge',
    unique_key='order_id',
) }}

-- Column discovery reads the source's CHANGES before the CI DT, whose creation would enable change
-- tracking, exists: the first build fails with 091930.
select o.order_id, o.amount
from {{ changes(source('raw', 'untracked'), 'default') }} as o
