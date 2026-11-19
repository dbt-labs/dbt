{{
    config(
        materialized='table',
    )
}}

select
    1 as id,
    'test' as name,
    cast(current_timestamp() as timestamp_ntz(6)) as created_at
