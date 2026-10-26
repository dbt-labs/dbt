{# The test's first command: creates the tables the rest of the test reads. #}
{% macro seed_bigquery() %}
  {% set s = target.schema %}
  {% do run_query("create schema if not exists `" ~ target.database ~ "`.`" ~ s ~ "`") %}
  {% do run_query("create or replace table " ~ s ~ ".a (val int)") %}
  {% for table in ["source_bulk", "source_field", "source_query"] %}
    {% do run_query("create table " ~ s ~ "." ~ table ~ " (id STRING, updated_at TIMESTAMP)") %}
    {% do run_query("insert into " ~ s ~ "." ~ table ~ " (id, updated_at) values ('1', CURRENT_TIMESTAMP())") %}
  {% endfor %}
{% endmacro %}
