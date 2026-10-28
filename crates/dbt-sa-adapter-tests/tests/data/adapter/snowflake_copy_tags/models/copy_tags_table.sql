{{ config(
    materialized = 'table',
    copy_grants = true,
    copy_tags = true
) }}

select 1 as id
