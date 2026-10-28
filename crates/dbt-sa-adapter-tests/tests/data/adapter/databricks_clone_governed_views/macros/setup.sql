{% macro create_policy_functions() %}
  {% set prefix = target.database ~ '.' ~ target.schema %}
  {% do run_query('create schema if not exists ' ~ prefix) %}
  {% do run_query("create or replace function " ~ prefix ~ ".region_a_filter(region string) returns boolean return region = 'a'") %}
  {% do run_query("create or replace function " ~ prefix ~ ".password_mask(password string) returns string return '*****'") %}
{% endmacro %}
