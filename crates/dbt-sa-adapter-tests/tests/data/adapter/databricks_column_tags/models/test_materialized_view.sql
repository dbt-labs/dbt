{{ config(materialized='materialized_view') }}

SELECT 
    id,
    name,
    email,
    amount
FROM {{ ref('a') }}

