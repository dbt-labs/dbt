{{ config(
    materialized='dynamic_table',
    snowflake_warehouse='FUSION_SLT_WAREHOUSE',
) }}

select order_id, customer_id, amount, false as is_deleted
from {{ source('raw', 'orders') }}
