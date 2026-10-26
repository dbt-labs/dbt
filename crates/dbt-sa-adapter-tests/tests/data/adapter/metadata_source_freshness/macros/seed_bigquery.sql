{# The test's first command: creates the tables the rest of the test reads. #}
{% macro seed_bigquery() %}
  {% set s = target.schema %}
  {% do run_query("create schema if not exists `" ~ target.database ~ "`.`" ~ s ~ "`") %}
  {% do run_query("create or replace table " ~ s ~ ".a (val int)") %}
  {% do run_query("create table " ~ s ~ ".source (id STRING, name STRING, amount DECIMAL(10,2))") %}
{% endmacro %}
