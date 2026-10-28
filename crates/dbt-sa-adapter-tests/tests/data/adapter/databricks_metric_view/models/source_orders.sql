{{ config(materialized='table') }}

select 1 as id, 100 as revenue, 'completed' as status, '2024-01-01' as order_date
union all
select 2 as id, 200 as revenue, 'pending' as status, '2024-01-02' as order_date
union all
select 3 as id, 150 as revenue, 'completed' as status, '2024-01-03' as order_date
