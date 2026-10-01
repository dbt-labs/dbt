{% macro snowflake__get_drop_dynamic_table_sql(relation) %}
    drop dynamic table if exists {{ relation }}
{% endmacro %}
