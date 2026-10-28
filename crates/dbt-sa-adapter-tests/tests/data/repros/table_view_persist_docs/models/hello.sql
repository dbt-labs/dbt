{{ config(
    materialized='table',
    persist_docs={ "columns": true, "relation": true }
) }}

select 1 as id, "foo" as name
