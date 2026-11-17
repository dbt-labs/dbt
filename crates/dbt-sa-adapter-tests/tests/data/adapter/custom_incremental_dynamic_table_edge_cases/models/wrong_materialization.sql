{{ config(
    materialized='table',
    refresh_mode='CUSTOM_INCREMENTAL',
) }}

-- Only dynamic_table models build CI DTs: dbt warns (dbt1111) and builds a table.
select 1 as order_id
