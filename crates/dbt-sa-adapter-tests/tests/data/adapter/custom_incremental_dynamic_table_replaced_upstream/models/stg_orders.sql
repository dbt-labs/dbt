{{ config(materialized='table', change_tracking=true) }}

select order_id, amount from {{ source('raw', 'orders') }}
