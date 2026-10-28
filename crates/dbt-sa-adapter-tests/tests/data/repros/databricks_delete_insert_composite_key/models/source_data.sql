{{
    config(
        materialized='table',
        file_format='delta'
    )
}}

select 1 as id, 'us-east' as region, 100 as value
union all select 2 as id, 'us-east' as region, 200 as value
union all select 3 as id, 'us-west' as region, 300 as value
