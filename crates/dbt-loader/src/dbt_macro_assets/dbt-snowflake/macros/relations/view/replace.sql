{% macro snowflake__get_replace_view_sql(relation, sql) %}
    {{ snowflake__create_view_as(relation, sql) }}
{% endmacro %}
