{{ config(materialized='table') }}

select 1 as id, 'a'::varchar(10) as name
