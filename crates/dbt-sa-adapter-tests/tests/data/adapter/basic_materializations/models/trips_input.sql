{{ config(materialized='ephemeral') }}

select id, zone, trips from {{ ref('trips') }}
