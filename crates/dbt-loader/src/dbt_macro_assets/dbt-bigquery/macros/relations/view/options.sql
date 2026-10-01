{% macro bigquery_view_options(config, node) %}
  {% set opts = adapter.get_view_options(config, node) %}
  {%- do return(bigquery_options(opts)) -%}
{%- endmacro -%}
