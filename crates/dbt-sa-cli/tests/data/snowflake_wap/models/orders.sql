{{ config(
    materialized='table',
    wap=true,
    wap_retain_failed=var('wap_retain_failed', none),
    alias=var('prefix') ~ '_ORDERS',
    transient=var('transient', false)
) }}

{% if var('transformation_error', false) %}
select to_number('WAP_TRANSFORM_FAILURE')::integer as id
{% elif var('self_read', false) %}
select (id + 1)::integer as id from {{ this }}
{% else %}
select {{ var('audit_value', 1) }}::integer as id
union all
select 2::integer as id
{% endif %}
