{% macro snowflake__get_replace_table_sql(relation, sql) %}
    {{ snowflake__create_table_as(False, relation, sql) }}
{% endmacro %}
