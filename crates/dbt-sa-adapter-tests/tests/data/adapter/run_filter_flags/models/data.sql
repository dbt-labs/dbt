{{ config(
    materialized = 'table',
    event_time='created_at'
) }}

select 1 as id, '2025-01-01' as created_at
union all
select 2 as id, '2025-01-02' as created_at
union all
select 3 as id, '2025-01-03' as created_at
union all
select 4 as id, '2025-01-04' as created_at
union all
select 5 as id, '2025-01-05' as created_at