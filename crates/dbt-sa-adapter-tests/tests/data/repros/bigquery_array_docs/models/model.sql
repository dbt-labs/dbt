{{ config(
    materialized="table"
) }}
select
    1 as id,
    [1,2,3] as my_array