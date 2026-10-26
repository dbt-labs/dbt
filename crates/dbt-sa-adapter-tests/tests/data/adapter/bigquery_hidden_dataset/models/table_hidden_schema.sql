{{
    config(
        materialized='table',
    )
}}

select 1 as id, 'hello' as value
