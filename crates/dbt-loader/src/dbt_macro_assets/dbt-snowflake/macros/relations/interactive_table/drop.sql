-- funcsign: (relation) -> string
{% macro snowflake__get_drop_interactive_table_sql(relation) %}
    drop table if exists {{ relation }}
{% endmacro %}
