{{ config(
    materialized='table',
    wap=true,
    wap_retain_failed=var('wap_retain_failed', none),
    alias=var('prefix') ~ '_ORDERS',
    transient=var('transient', false),
    post_hook="{{ wap_fixture_post_hook() }}"
) }}

{% if var('header_session', false) %}
{% call set_sql_header(config) %}
set wap_header_id = 41;
{% endcall %}
{% endif %}

{% if var('transformation_error', false) %}
select to_number('WAP_TRANSFORM_FAILURE')::integer as id
{% elif var('self_read', false) %}
select (id + 1)::integer as id from {{ this }}
{% elif var('header_session', false) %}
select $wap_header_id::integer as id
{% else %}
select {{ var('audit_value', 1) }}::integer as id
union all
select 2::integer as id
{% endif %}
