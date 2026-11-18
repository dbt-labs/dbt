{{ config(materialized='table') }}

SELECT 
    id,
    name,
    email,
    amount
FROM {{ ref('a') }}

