{{ config(materialized='table', contract={"enforced": true}) }}

select 1 as id, 'Alice' as name
