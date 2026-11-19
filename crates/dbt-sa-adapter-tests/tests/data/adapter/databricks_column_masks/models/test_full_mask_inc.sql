{{ config(
    materialized = 'incremental',
    incremental_strategy = 'merge',
    unique_key = 'id',
) }}

select cast(1 as bigint) as id, 'hello' as name, 'john.doe@example.com' as email,
'password123' as password