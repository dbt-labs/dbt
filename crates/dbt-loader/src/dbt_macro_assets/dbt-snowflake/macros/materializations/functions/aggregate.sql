{% macro snowflake__aggregate_function_python(target_relation) %}
    {{ get_aggregate_function_create_replace_signature(target_relation) }}
    {# conveniently we can reuse the sql scalar function body #}
    {{ scalar_function_body_sql() }}
{% endmacro %}

{% macro snowflake__aggregate_function_javascript(target_relation) %}
    {% do exceptions.raise_compiler_error('JS aggregate functions not supported in snowflake') %}
{% endmacro %}
