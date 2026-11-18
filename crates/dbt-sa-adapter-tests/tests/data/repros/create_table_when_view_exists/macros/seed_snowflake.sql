{# The test's first command: creates the relations the rest of the test reads. #}
{% macro seed_snowflake() %}
  {% do run_query("create schema if not exists " ~ target.database ~ "." ~ target.schema) %}
  {% do run_query("create table " ~ target.schema ~ ".source (id VARCHAR(255))") %}
  {% do run_query("insert into " ~ target.schema ~ ".source (id) values ('1')") %}
  {% do run_query("create view " ~ target.schema ~ ".source_view as select * from " ~ target.schema ~ ".source") %}
{% endmacro %}
