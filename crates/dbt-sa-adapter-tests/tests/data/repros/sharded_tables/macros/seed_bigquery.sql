{# The test's first command: creates the tables the rest of the test reads. #}
{% macro seed_bigquery() %}
  {% set s = target.schema %}
  {% do run_query("create schema if not exists `" ~ target.database ~ "`.`" ~ s ~ "`") %}
  {% do run_query("create or replace table " ~ s ~ ".a (val int)") %}
  {% do run_query("create table " ~ s ~ ".favourite_animals_20250101 (name STRING, species STRING, age INTEGER)") %}
  {% do run_query("insert into " ~ s ~ ".favourite_animals_20250101 (name, species, age) values ('Spike', 'dog', 5)") %}
  {% do run_query("create table " ~ s ~ ".favourite_animals_20250102 (name STRING, species STRING, age INTEGER)") %}
  {% do run_query("insert into " ~ s ~ ".favourite_animals_20250102 (name, species, age) values ('Lulu', 'cat', 4)") %}
{% endmacro %}
