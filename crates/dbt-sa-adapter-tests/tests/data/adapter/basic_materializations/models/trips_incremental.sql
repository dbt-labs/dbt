{{ config(materialized='incremental', unique_key='id', incremental_strategy='delete+insert') }}

-- Reprocess all three IDs on each run; the second build must not duplicate them.
select id, zone, trips from {{ ref('trips_input') }}
