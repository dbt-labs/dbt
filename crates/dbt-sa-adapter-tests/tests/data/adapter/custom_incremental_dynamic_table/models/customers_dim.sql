{{ config(materialized='ephemeral') }}

select customer_id, region from {{ source('raw', 'customers') }}
