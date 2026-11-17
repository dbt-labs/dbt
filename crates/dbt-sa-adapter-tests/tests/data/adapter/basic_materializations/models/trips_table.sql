{{ config(materialized='table') }}

select id, zone, trips from {{ ref('trips_input') }}
