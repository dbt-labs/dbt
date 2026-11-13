{# The test's first command: creates the tables the rest of the test reads. #}
{% macro seed_redshift() %}
  {% set s = target.schema %}
  {% do run_query("create schema if not exists " ~ s) %}
  {% do run_query("drop table if exists " ~ s ~ ".a") %}
  {% do run_query("create table " ~ s ~ ".a (val int)") %}
  {% do run_query("create table " ~ s ~ ".source (id VARCHAR(255), name VARCHAR(255), amount DECIMAL(10,2))") %}
  {% do run_query("insert into " ~ s ~ ".source (id, name, amount) values ('1', 'Name1', 100.50)") %}
{% endmacro %}
