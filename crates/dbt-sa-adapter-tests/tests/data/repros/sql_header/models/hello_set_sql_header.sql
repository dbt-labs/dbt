{{ config(
    materialized='table'
) }}

{% call set_sql_header(config) %}
 ALTER SESSION SET TIMEZONE = 'Asia/Kolkata';
{%- endcall %}

select 'Hello World' as message
