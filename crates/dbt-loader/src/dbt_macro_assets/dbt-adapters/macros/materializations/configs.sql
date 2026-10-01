-- funcsign: (config) -> string
{% macro set_sql_header(config) -%}
  {{ config.set('sql_header', caller()) }}
{%- endmacro %}


-- funcsign: () -> bool
{% macro should_full_refresh() %}
  {% set config_full_refresh = config.get('full_refresh') %}
  {% if config_full_refresh is none %}
    {% set config_full_refresh = flags.FULL_REFRESH %}
  {% endif %}
  {% do return(config_full_refresh) %}
{% endmacro %}


-- ai
-- funcsign: () -> bool
{% macro should_store_failures() %}
  {% set config_store_failures = config.get('store_failures') %}
  {% if config_store_failures is none %}
    {% set config_store_failures = flags.STORE_FAILURES %}
  {% endif %}
  {% do return(config_store_failures) %}
{% endmacro %}
