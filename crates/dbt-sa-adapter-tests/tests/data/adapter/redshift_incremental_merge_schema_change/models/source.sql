{{ config(materialized='table') }}

select 1 as id, cast('alpha' as varchar(20)) as name
union all
select 2 as id, cast('beta' as varchar(20)) as name
