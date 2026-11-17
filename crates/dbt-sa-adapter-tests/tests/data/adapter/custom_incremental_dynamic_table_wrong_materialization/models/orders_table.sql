{{ config(
    materialized='table',
    refresh_mode='CUSTOM_INCREMENTAL',
) }}

select 1 as order_id
