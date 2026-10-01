{% macro snowflake__get_drop_view_sql(relation) %}
    drop view if exists {{ relation }} cascade
{% endmacro %}
