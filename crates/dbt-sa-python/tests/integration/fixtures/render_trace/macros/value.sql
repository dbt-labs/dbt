{% macro probe_value(value) %}{{ return(value) }}{% endmacro %}

{% macro default__trace_column(column) -%}
    {{ adapter.quote(column) }}
{%- endmacro %}
