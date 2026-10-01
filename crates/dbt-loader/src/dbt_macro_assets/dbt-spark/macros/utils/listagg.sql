{% macro spark__listagg(measure, delimiter_text, order_by_clause, limit_num) -%}

  {% if order_by_clause %}
    {{ exceptions.warn("order_by_clause is not supported for listagg on Spark/Databricks") }}
  {% endif %}

  {% set collect_list %} collect_list({{ measure }}) {% endset %}

  {% set limited %} slice({{ collect_list }}, 1, {{ limit_num }}) {% endset %}

  {% set collected = limited if limit_num else collect_list %}

  {% set final %} array_join({{ collected }}, {{ delimiter_text }}) {% endset %}

  {% do return(final) %}

{%- endmacro %}
