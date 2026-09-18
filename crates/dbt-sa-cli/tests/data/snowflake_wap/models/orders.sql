{{ config(
    materialized='table',
    wap=true,
    alias=var('prefix') ~ '_ORDERS',
    transient=var('transient', false)
) }}

select {{ var('audit_value', 1) }}::integer as id
union all
select 2::integer as id
