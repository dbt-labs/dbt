{% macro enable_predictive_optimization() %}
  {% do run_query("create schema if not exists `" ~ target.database ~ "`.`" ~ target.schema ~ "`") %}
  {% do run_query("alter schema `" ~ target.database ~ "`.`" ~ target.schema ~ "` enable predictive optimization") %}
{% endmacro %}

{% macro disable_predictive_optimization() %}
  {% do run_query("alter schema `" ~ target.database ~ "`.`" ~ target.schema ~ "` disable predictive optimization") %}
{% endmacro %}
