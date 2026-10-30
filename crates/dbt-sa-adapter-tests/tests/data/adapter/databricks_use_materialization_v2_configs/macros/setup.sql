{% macro set_up() %}
  {% do adapter.dispatch("set_up")() %}
{% endmacro %}

{% macro default__set_up() %}
{% endmacro %}

{% macro databricks__set_up() %}
  {% do adapter.execute('CREATE SCHEMA IF NOT EXISTS `' ~ target.database ~ '`.`' ~ target.schema ~ '`') %}
  {% do adapter.execute("create or replace view `" ~ target.database ~ "`.`" ~ target.schema ~ "`.`v_view` as ( select 1 as id, 'a' as val)") %}
  {% do adapter.execute("create or replace table `" ~ target.database ~ "`.`" ~ target.schema ~ "`.`t_incremental` as ( select 1 as id )") %}
  {% do adapter.execute("create or replace table `" ~ target.database ~ "`.`" ~ target.schema ~ "`.`t_table` as ( select 1 as id )") %}
{% endmacro %}
