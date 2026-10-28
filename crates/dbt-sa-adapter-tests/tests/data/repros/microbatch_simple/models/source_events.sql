{{ config(materialized='table', event_time='event_ts') }}

select cast('2025-01-01 12:00:00' as timestamp) as event_ts, 1 as id
union all
select cast('2025-01-02 12:00:00' as timestamp) as event_ts, 2 as id
union all
select cast('2025-01-03 12:00:00' as timestamp) as event_ts, 3 as id
union all
select cast('2025-01-04 12:00:00' as timestamp) as event_ts, 4 as id
