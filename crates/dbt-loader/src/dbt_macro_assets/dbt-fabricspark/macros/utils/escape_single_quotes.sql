{# /* Spark/Databricks uses a single backslash: they're -> they\'re. The second backslash is to escape it from Jinja */ #}
{% macro fabricspark__escape_single_quotes(expression) -%}
{{ expression | replace("'","\\'") }}
{%- endmacro %}
