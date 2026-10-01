-- funcsign: (string) -> agate_table
{% macro salesforce__validate_sql(sql) -%}
{#
    TODO: This can potentially be implemented with a DataTransform Validate call.
    However, the driver doesn't provide a way to "dry-run" a query.
#}
{{ exceptions.raise_not_implemented('validate_sql macro not implemented for adapter salesforce') }}
{% endmacro %}
