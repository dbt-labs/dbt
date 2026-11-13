{% macro trino__listagg(measure, delimiter_text, order_by_clause, limit_num) -%}
    {% set collect_list %} array_agg({{ measure }} {% if order_by_clause -%}{{ order_by_clause }}{%- endif %}) {% endset %}
    {% set limited %} slice({{ collect_list }}, 1, {{ limit_num }}) {% endset %}
    {% set collected = limited if limit_num else collect_list %}
    {% set final %} array_join({{ collected }}, {{ delimiter_text }}) {% endset %}
    {% do return(final) %}
{%- endmacro %}
