{{ config(materialized='materialized_view', alias=('governed_mv_prod' if var('phase', 'ci') == 'prod' else 'governed_mv')) }}
select 'a' as region, 1 as id
union all
select 'b' as region, 2 as id
