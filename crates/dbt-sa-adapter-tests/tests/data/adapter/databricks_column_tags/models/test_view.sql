{{ config(materialized='view') }}

SELECT 
    id,
    name,
    email,
    amount
FROM {{ ref('a') }}

