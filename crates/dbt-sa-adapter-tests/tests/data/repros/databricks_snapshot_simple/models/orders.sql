{{
    config(
        materialized='table',
        file_format='delta'
    )
}}

-- Base orders table
with source_data as (
    select 1 as id, 'pending' as status, cast('2025-01-01 09:00:00' as timestamp) as updated_at
    union all 
    select 2 as id, 'shipped' as status, cast('2025-01-01 09:00:00' as timestamp) as updated_at
)

select 
    id,
    status,
    updated_at
from source_data

