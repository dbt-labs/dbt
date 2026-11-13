{{ config(
    materialized = 'table'
) }}

select
    'abc123' as id,
    'password123' as password;
