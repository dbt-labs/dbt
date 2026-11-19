--- test related macros
{% macro set_up() %}
  {% do adapter.dispatch("set_up")() %}
{% endmacro %}

{% macro default__set_up() %}
  {{ print("default") }}
{% endmacro %}

--- Databricks only persists column docs for delta
{% macro databricks__set_up() %}
  {{ print("databricks") }}
  {{ config(
    file_format='delta'
  ) }}
{% endmacro %}
