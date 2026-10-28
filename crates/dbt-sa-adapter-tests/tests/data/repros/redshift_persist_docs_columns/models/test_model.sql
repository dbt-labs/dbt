{{ config(
    materialized='table',
    persist_docs={ "columns": true, "relation": true }
) }}

SELECT
    1 AS id,
    'Alice' AS name,
    25 AS age,
    'Engineering' AS department

