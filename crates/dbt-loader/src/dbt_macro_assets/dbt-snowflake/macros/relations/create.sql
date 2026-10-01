{% macro snowflake__get_create_sql(relation, sql) %}

    {% if relation.is_interactive_table %}
        {{ snowflake__get_create_interactive_table_as_sql(relation, sql) }}

    {% elif relation.is_dynamic_table %}
        {{ snowflake__get_create_dynamic_table_as_sql(relation, sql) }}

    {% else %}
        {{ default__get_create_sql(relation, sql) }}

    {% endif %}

{% endmacro %}
