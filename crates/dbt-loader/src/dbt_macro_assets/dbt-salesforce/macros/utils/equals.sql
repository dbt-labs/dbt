{# See: https://developer.salesforce.com/docs/data/data-cloud-query-guide/references/dc-sql-reference/comparison.html#null-expressions #}

-- funcsign: (string, string) -> string
{% macro salesforce__equals(expr1, expr2) -%}
{%- if adapter.behavior.enable_truthy_nulls_equals_macro.no_warn %}
    ({{ expr1 }} IS NOT DISTINCT FROM {{ expr2 }})
{%- else -%}
    ({{ expr1 }} = {{ expr2 }})
{%- endif %}
{% endmacro %}
