{% macro bigquery_table_options(config, node, temporary) %}
  {% set opts = adapter.get_table_options(config, node, temporary) %}
  {%- do return(bigquery_options(opts)) -%}
{%- endmacro -%}
