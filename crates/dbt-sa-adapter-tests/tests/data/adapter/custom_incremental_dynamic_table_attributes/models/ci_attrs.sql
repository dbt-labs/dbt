{%- set fq = target.database ~ '.' ~ target.schema -%}
{{ config(
    materialized='dynamic_table',
    snowflake_warehouse='FUSION_SLT_WAREHOUSE',
    snowflake_initialization_warehouse='FUSION_ADAPTER_TESTING',
    target_lag='1 hour',
    refresh_mode='CUSTOM_INCREMENTAL',
    incremental_strategy='merge',
    unique_key='order_id',
    cluster_by=[var('cluster_key')],
    table_tag=fq ~ ".mytag = 'v1'",
    row_access_policy=fq ~ '.rap on (order_id)',
    copy_grants=true,
    persist_docs={'relation': true},
) }}

-- An update arrives as a DELETE and an INSERT of the same key; keep the INSERT.
select o.order_id, o.amount
from {{ changes(source('raw', 'orders'), 'default') }} as o
qualify row_number() over (
    partition by o.order_id
    order by case when o.METADATA$ACTION = 'INSERT' then 0 else 1 end
) = 1
