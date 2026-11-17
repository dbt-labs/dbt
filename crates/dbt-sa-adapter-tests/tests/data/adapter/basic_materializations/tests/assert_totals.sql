select 'table_count' as failure
where (select count(*) from {{ ref('trips_table') }}) <> 3
union all
select 'view_count' as failure
where (select count(*) from {{ ref('zone_totals') }}) <> 2
union all
select 'view_total' as failure
where coalesce((select sum(trips) from {{ ref('zone_totals') }}), 0) <> 6
union all
select 'incremental_count' as failure
where (select count(*) from {{ ref('trips_incremental') }}) <> 3
union all
select 'incremental_total' as failure
where coalesce((select sum(trips) from {{ ref('trips_incremental') }}), 0) <> 6
