{# The test's first command: creates the tables the rest of the test reads. #}
{% macro seed_snowflake() %}
  {% do run_query("create schema if not exists " ~ target.database ~ "." ~ target.schema) %}
  {% do run_query("create or replace table " ~ target.schema ~ ".a (val int)") %}
  {% do run_query("create table " ~ target.schema ~ ".source (id VARCHAR(255), name VARCHAR(255), amount NUMERIC(10,2))") %}
{% endmacro %}
