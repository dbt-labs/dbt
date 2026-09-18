{{ config(
    materialized='table',
    wap=true,
    alias=var('prefix') ~ '_ORDERS',
    transient=var('transient', false)
) }}

{% if var('transformation_error', false) %}
select to_number('WAP_TRANSFORM_FAILURE')::integer as id
{% else %}
select {{ var('audit_value', 1) }}::integer as id
union all
select 2::integer as id
{% endif %}
