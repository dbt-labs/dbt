{{ config(
    materialized='view',
    persist_docs={ "columns": true, "relation": true }
) }}

select * from {{ ref('hello') }}
